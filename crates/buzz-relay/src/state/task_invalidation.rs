//! Task HTTP writes invalidate live clients after the durable mutation commits.

use super::{AppState, CommunityId, ConnectionManager, HashMap, Uuid, WsMessage};
use buzz_core::TenantContext;
use buzz_pubsub::conn_control::ConnControl;
use std::time::Duration;

impl ConnectionManager {
    /// Snapshot authenticated identities without retaining DashMap guards across await.
    fn task_recipients(&self, community_id: CommunityId) -> HashMap<Vec<u8>, Vec<Uuid>> {
        let mut recipients: HashMap<Vec<u8>, Vec<Uuid>> = HashMap::new();
        for entry in self.connections.iter() {
            if entry.community_id != community_id || entry.cancel.is_cancelled() {
                continue;
            }
            if let Ok(identity) = entry.authenticated_pubkey.read() {
                if let Some(pubkey) = identity.as_ref() {
                    recipients
                        .entry(pubkey.clone())
                        .or_default()
                        .push(*entry.key());
                }
            }
        }
        recipients
    }

    /// Recheck the authenticated recipient after the asynchronous permission lookup.
    fn send_task_invalidation(
        &self,
        connection_id: Uuid,
        community_id: CommunityId,
        pubkey: &[u8],
        frame: WsMessage,
    ) {
        let Some(entry) = self.connections.get(&connection_id) else {
            return;
        };
        if entry.community_id != community_id || entry.cancel.is_cancelled() {
            return;
        }
        let Ok(identity) = entry.authenticated_pubkey.read() else {
            return;
        };
        if identity.as_deref() != Some(pubkey) {
            return;
        }
        if entry.ctrl_tx.try_send(frame).is_err() {
            // A slow client must reconnect and refresh instead of silently
            // retaining a stale task while its priority queue remains full.
            entry.cancel.cancel();
            metrics::counter!("buzz_tasks_invalidation_dropped_total").increment(1);
        }
    }
}

impl AppState {
    async fn task_recipient_is_relay_member(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
    ) -> Result<bool, buzz_db::DbError> {
        if !self.config.require_relay_membership {
            return Ok(true);
        }
        if self
            .db
            .is_relay_member_writer(community_id, &hex::encode(pubkey))
            .await?
        {
            return Ok(true);
        }
        if !self.config.allow_nip_oa_auth {
            return Ok(false);
        }
        let owner = self
            .db
            .get_agent_channel_policy(community_id, pubkey)
            .await?
            .and_then(|(_, owner)| owner);
        match owner {
            Some(owner) => {
                self.db
                    .is_relay_member_writer(community_id, &hex::encode(owner))
                    .await
            }
            None => Ok(false),
        }
    }

    /// Notify local and remote authenticated sockets after a committed task write.
    ///
    /// Channel advisories carry only their UUID and use a fresh writer-backed
    /// access read, never a cached allow. Community advisories carry `null`.
    /// No task contents or task identifiers are broadcast. Both local fanout
    /// and Redis publication are bounded; advisory failure cannot turn a
    /// committed HTTP write into an error.
    pub(crate) async fn invalidate_tasks(
        &self,
        community_id: CommunityId,
        channel_id: Option<Uuid>,
    ) {
        self.deliver_task_invalidation(community_id, channel_id)
            .await;

        let tenant = TenantContext::resolved(community_id, "task-invalidation.internal");
        let command = ConnControl::InvalidateTasks {
            channel_id,
            origin_generation: self.task_invalidation_generation,
        };
        match tokio::time::timeout(
            Duration::from_secs(1),
            self.pubsub.publish_conn_control(&tenant, &command),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(%community_id, %error, "task invalidation publish failed");
                metrics::counter!("buzz_tasks_invalidation_publish_errors_total").increment(1);
            }
            Err(_) => {
                tracing::warn!(%community_id, "task invalidation publish timed out");
                metrics::counter!("buzz_tasks_invalidation_publish_timeouts_total").increment(1);
            }
        }
    }

    /// Apply a task invalidation only to sockets held by this relay process.
    /// Cross-node consumers call this without re-publishing.
    pub async fn deliver_task_invalidation(
        &self,
        community_id: CommunityId,
        channel_id: Option<Uuid>,
    ) {
        let recipients = self.conn_manager.task_recipients(community_id);
        let fanout = async {
            let frame = WsMessage::Text(
                crate::protocol::RelayMessage::tasks_sync_required(channel_id.as_ref()).into(),
            );
            for (pubkey, connections) in recipients {
                match self
                    .task_recipient_is_relay_member(community_id, &pubkey)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => {
                        tracing::warn!(%community_id, %error, "task invalidation relay membership lookup failed");
                        metrics::counter!("buzz_tasks_invalidation_access_errors_total")
                            .increment(1);
                        continue;
                    }
                }
                if let Some(channel_id) = channel_id {
                    let channels = match self
                        .db
                        .get_accessible_channel_ids(community_id, &pubkey)
                        .await
                    {
                        Ok(channels) => channels,
                        Err(error) => {
                            tracing::warn!(%community_id, %error, "task invalidation access lookup failed");
                            metrics::counter!("buzz_tasks_invalidation_access_errors_total")
                                .increment(1);
                            continue;
                        }
                    };
                    if !channels.contains(&channel_id) {
                        continue;
                    }
                }
                for connection_id in connections {
                    self.conn_manager.send_task_invalidation(
                        connection_id,
                        community_id,
                        &pubkey,
                        frame.clone(),
                    );
                }
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), fanout)
            .await
            .is_err()
        {
            tracing::warn!(%community_id, "task invalidation fanout timed out");
            metrics::counter!("buzz_tasks_invalidation_timeouts_total").increment(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicU8, Arc};
    use tokio::sync::{mpsc, Mutex};
    use tokio_util::sync::CancellationToken;

    fn connection(
        manager: &ConnectionManager,
        community: CommunityId,
        pubkey: &[u8],
    ) -> (Uuid, mpsc::Receiver<WsMessage>, CancellationToken) {
        let id = Uuid::new_v4();
        let (tx, _rx) = mpsc::channel(1);
        let (ctrl, rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        manager.register(
            id,
            tx,
            ctrl,
            None,
            cancel.clone(),
            community,
            Arc::new(AtomicU8::new(0)),
            Arc::new(Mutex::new(HashMap::new())),
            3,
        );
        manager.set_authenticated_pubkey(id, pubkey.to_vec());
        (id, rx, cancel)
    }

    #[test]
    fn identity_change_after_snapshot_cannot_receive_previous_recipients_frame() {
        let manager = ConnectionManager::new();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        let (id, mut rx, _) = connection(&manager, community, &[1; 32]);
        assert_eq!(
            manager.task_recipients(community).get(&vec![1; 32]),
            Some(&vec![id])
        );
        manager.set_authenticated_pubkey(id, vec![2; 32]);
        manager.send_task_invalidation(id, community, &[1; 32], WsMessage::Text("signal".into()));
        assert!(rx.try_recv().is_err());
        manager.send_task_invalidation(id, community, &[2; 32], WsMessage::Text("signal".into()));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn full_authorized_control_queue_forces_reconnect_recovery() {
        let manager = ConnectionManager::new();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        let (id, _rx, cancel) = connection(&manager, community, &[1; 32]);
        manager.send_task_invalidation(id, community, &[1; 32], WsMessage::Text("first".into()));
        assert!(!cancel.is_cancelled());
        manager.send_task_invalidation(id, community, &[1; 32], WsMessage::Text("second".into()));
        assert!(cancel.is_cancelled());
        assert!(manager.task_recipients(community).is_empty());
    }
}
