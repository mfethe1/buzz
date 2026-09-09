//! Task HTTP writes invalidate live clients after the durable mutation commits.

use super::{AppState, CommunityId, ConnectionManager, HashMap, Uuid, WsMessage};
use buzz_core::TenantContext;
use buzz_pubsub::conn_control::ConnControl;
use std::{collections::HashSet, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TaskRecipientScope {
    pubkey: Vec<u8>,
    delegation_owner: Option<Vec<u8>>,
}

#[derive(Clone, Copy)]
struct TaskRecipientLease {
    connection_id: Uuid,
    generation: Uuid,
}

impl ConnectionManager {
    /// Group equal session permissions without retaining guards across await.
    fn task_recipients(
        &self,
        community_id: CommunityId,
    ) -> HashMap<TaskRecipientScope, Vec<TaskRecipientLease>> {
        let mut recipients: HashMap<TaskRecipientScope, Vec<TaskRecipientLease>> = HashMap::new();
        for entry in self.connections.iter() {
            if entry.community_id != community_id || entry.cancel.is_cancelled() {
                continue;
            }
            if let Ok(identity) = entry.authenticated_session.read() {
                if let Some(identity) = identity.as_ref() {
                    recipients
                        .entry(TaskRecipientScope {
                            pubkey: identity.pubkey.clone(),
                            delegation_owner: identity.delegation_owner.clone(),
                        })
                        .or_default()
                        .push(TaskRecipientLease {
                            connection_id: *entry.key(),
                            generation: identity.generation,
                        });
                }
            }
        }
        recipients
    }

    /// Fence the complete authenticated session after asynchronous authorization.
    fn send_task_invalidation(
        &self,
        lease: TaskRecipientLease,
        community_id: CommunityId,
        scope: &TaskRecipientScope,
        frame: WsMessage,
    ) {
        self.finish_task_invalidation(lease, community_id, scope, Some(frame));
    }

    /// A missing advisory forces recovery, fenced to the captured session.
    fn finish_task_invalidation(
        &self,
        lease: TaskRecipientLease,
        community_id: CommunityId,
        scope: &TaskRecipientScope,
        frame: Option<WsMessage>,
    ) {
        let Some(entry) = self.connections.get(&lease.connection_id) else {
            return;
        };
        if entry.community_id != community_id || entry.cancel.is_cancelled() {
            return;
        }
        let Ok(identity) = entry.authenticated_session.read() else {
            return;
        };
        let Some(identity) = identity.as_ref() else {
            return;
        };
        if identity.pubkey != scope.pubkey
            || identity.delegation_owner != scope.delegation_owner
            || identity.generation != lease.generation
        {
            return;
        }
        let delivered = frame.is_some_and(|frame| entry.ctrl_tx.try_send(frame).is_ok());
        if !delivered {
            entry.cancel.cancel();
            metrics::counter!("buzz_tasks_invalidation_dropped_total").increment(1);
        }
    }
}

impl AppState {
    async fn task_recipient_is_relay_member(
        &self,
        community_id: CommunityId,
        scope: &TaskRecipientScope,
    ) -> Result<bool, buzz_db::DbError> {
        if !self.config.require_relay_membership {
            return Ok(true);
        }
        if self
            .db
            .is_relay_member_writer(community_id, &hex::encode(&scope.pubkey))
            .await?
        {
            return Ok(true);
        }
        if !self.config.allow_nip_oa_auth {
            return Ok(false);
        }
        // A persisted owner association cannot substitute for a verified
        // delegation on this connection. Owner membership is re-read on writer.
        match scope.delegation_owner.as_deref() {
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
        let mut completed = HashSet::new();
        let fanout = async {
            let frame = WsMessage::Text(
                crate::protocol::RelayMessage::tasks_sync_required(channel_id.as_ref()).into(),
            );
            for (scope, connections) in &recipients {
                match self
                    .task_recipient_is_relay_member(community_id, scope)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        completed.insert(scope.clone());
                        continue;
                    }
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
                        .get_accessible_channel_ids(community_id, &scope.pubkey)
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
                        completed.insert(scope.clone());
                        continue;
                    }
                }
                for &lease in connections {
                    self.conn_manager.send_task_invalidation(
                        lease,
                        community_id,
                        scope,
                        frame.clone(),
                    );
                }
                completed.insert(scope.clone());
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), fanout)
            .await
            .is_err()
        {
            tracing::warn!(%community_id, "task invalidation fanout timed out");
            metrics::counter!("buzz_tasks_invalidation_timeouts_total").increment(1);
        }
        // A failed lookup is not a denial. Recover only unresolved sessions;
        // never disclose a channel identifier without successful authorization.
        for (scope, connections) in recipients {
            if completed.contains(&scope) {
                continue;
            }
            for lease in connections {
                self.conn_manager
                    .finish_task_invalidation(lease, community_id, &scope, None);
            }
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

    fn snapshot(
        manager: &ConnectionManager,
        community: CommunityId,
    ) -> (TaskRecipientScope, TaskRecipientLease) {
        let recipients = manager.task_recipients(community);
        assert_eq!(recipients.len(), 1);
        let (scope, leases) = recipients.into_iter().next().unwrap();
        assert_eq!(leases.len(), 1);
        (scope, leases[0])
    }

    #[test]
    fn identity_change_after_snapshot_cannot_receive_previous_recipients_frame() {
        let manager = ConnectionManager::new();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        let (id, mut rx, _) = connection(&manager, community, &[1; 32]);
        let (old_scope, old_lease) = snapshot(&manager, community);
        manager.set_authenticated_pubkey(id, vec![2; 32]);
        manager.send_task_invalidation(
            old_lease,
            community,
            &old_scope,
            WsMessage::Text("old".into()),
        );
        assert!(rx.try_recv().is_err());
        let (scope, lease) = snapshot(&manager, community);
        manager.send_task_invalidation(lease, community, &scope, WsMessage::Text("current".into()));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn same_key_returning_to_prior_scope_cannot_reuse_an_old_authorization() {
        let manager = ConnectionManager::new();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        let (id, mut rx, _) = connection(&manager, community, &[1; 32]);
        let (old_scope, old_lease) = snapshot(&manager, community);
        manager.set_authenticated_session(id, vec![1; 32], Some(vec![2; 32]));
        let (delegated_scope, delegated_lease) = snapshot(&manager, community);
        manager.set_authenticated_pubkey(id, vec![1; 32]);
        manager.send_task_invalidation(
            old_lease,
            community,
            &old_scope,
            WsMessage::Text("old direct".into()),
        );
        manager.send_task_invalidation(
            delegated_lease,
            community,
            &delegated_scope,
            WsMessage::Text("old delegated".into()),
        );
        assert!(rx.try_recv().is_err());
        let (scope, lease) = snapshot(&manager, community);
        manager.send_task_invalidation(lease, community, &scope, WsMessage::Text("current".into()));
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn same_key_with_different_verified_owners_has_separate_permission_groups() {
        let manager = ConnectionManager::new();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        let (_direct, _rx, _) = connection(&manager, community, &[1; 32]);
        let (delegated, _rx2, _) = connection(&manager, community, &[1; 32]);
        manager.set_authenticated_session(delegated, vec![1; 32], Some(vec![2; 32]));
        let recipients = manager.task_recipients(community);
        assert_eq!(recipients.len(), 2);
        assert!(recipients
            .keys()
            .any(|scope| scope.delegation_owner.is_none()));
        assert!(recipients
            .keys()
            .any(|scope| scope.delegation_owner == Some(vec![2; 32])));
    }

    #[test]
    fn recovery_disconnect_is_fenced_to_the_captured_session_and_community() {
        let manager = ConnectionManager::new();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        let other_community = CommunityId::from_uuid(Uuid::new_v4());
        let (id, mut rx, cancel) = connection(&manager, community, &[1; 32]);
        let (_other_id, _other_rx, other_cancel) = connection(&manager, other_community, &[1; 32]);
        let (old_scope, old_lease) = snapshot(&manager, community);
        manager.set_authenticated_session(id, vec![1; 32], Some(vec![2; 32]));
        manager.set_authenticated_pubkey(id, vec![1; 32]);
        manager.finish_task_invalidation(old_lease, community, &old_scope, None);
        assert!(
            !cancel.is_cancelled(),
            "old generation cannot cancel a new session"
        );
        let (scope, lease) = snapshot(&manager, community);
        manager.finish_task_invalidation(lease, other_community, &scope, None);
        assert!(!cancel.is_cancelled(), "community must match the lease");
        manager.finish_task_invalidation(lease, community, &scope, None);
        assert!(
            cancel.is_cancelled(),
            "unresolved current session must reconnect"
        );
        assert!(
            !other_cancel.is_cancelled(),
            "other communities remain connected"
        );
        assert!(
            rx.try_recv().is_err(),
            "recovery carries no channel advisory"
        );
    }

    #[test]
    fn full_authorized_control_queue_forces_reconnect_recovery() {
        let manager = ConnectionManager::new();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        let (_id, _rx, cancel) = connection(&manager, community, &[1; 32]);
        let (scope, lease) = snapshot(&manager, community);
        manager.send_task_invalidation(lease, community, &scope, WsMessage::Text("first".into()));
        assert!(!cancel.is_cancelled());
        manager.send_task_invalidation(lease, community, &scope, WsMessage::Text("second".into()));
        assert!(cancel.is_cancelled());
        assert!(manager.task_recipients(community).is_empty());
    }
}
