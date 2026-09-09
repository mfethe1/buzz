//! Keep urgent socket control independent of bounded advisory authorization.

use super::{AppState, CommunityId, ConnControl, Uuid};
use buzz_pubsub::conn_control::ScopedConnControl;
use futures_util::{stream::FuturesUnordered, StreamExt};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::broadcast;

const MAX_PENDING_TASK_SCOPES: usize = 256;

impl AppState {
    /// Consume cross-pod commands without awaiting advisory database work.
    /// One active fanout and a bounded, duplicate-coalescing FIFO preserve
    /// urgent control responsiveness. Loss forces fresh authorization on reconnect.
    pub async fn run_connection_control(
        self: Arc<Self>,
        mut rx: broadcast::Receiver<ScopedConnControl>,
    ) {
        let mut pending: VecDeque<(CommunityId, Option<Uuid>)> = VecDeque::new();
        let mut active = FuturesUnordered::new();
        loop {
            tokio::select! {
                _ = active.next(), if !active.is_empty() => {}
                received = rx.recv() => match received {
                    Ok(ScopedConnControl { community_id, command: ConnControl::InvalidateTasks { channel_id, origin_generation } }) => {
                        if origin_generation != self.task_invalidation_generation {
                            let scope = (community_id, channel_id);
                            if !pending.contains(&scope) {
                                if pending.len() < MAX_PENDING_TASK_SCOPES {
                                    pending.push_back(scope);
                                } else {
                                    // Dropping an advisory without recovery could leave a
                                    // connected client stale forever. Close only its tenant.
                                    self.reconnect_after_control_loss(Some(community_id));
                                    pending.retain(|(community, _)| *community != community_id);
                                    metrics::counter!("buzz_task_control_overflow_total").increment(1);
                                }
                            }
                        }
                    }
                    Ok(scoped) => self.apply_conn_control(scoped).await,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        metrics::counter!("buzz_conn_control_lag_total").increment(n);
                        tracing::warn!("Connection-control consumer lost {n} commands; reconnecting sockets");
                        // The missed command might revoke any tenant/key. Its
                        // durable authorization is checked on the next connection.
                        self.reconnect_after_control_loss(None);
                        pending.clear();
                        active.clear();
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        self.reconnect_after_control_loss(None);
                        tracing::error!("Connection-control broadcast channel closed");
                        break;
                    }
                }
            }
            if active.is_empty() {
                if let Some((community, channel)) = pending.pop_front() {
                    let state = self.clone();
                    active.push(async move {
                        state.deliver_task_invalidation(community, channel).await;
                    });
                }
            }
        }
    }

    fn reconnect_after_control_loss(&self, community: Option<CommunityId>) {
        for entry in self.community_connections.connections.iter() {
            if community.is_none_or(|id| id == entry.value().0) {
                // A recovery close must never claim the community was deleted.
                entry.value().1.cancel.cancel();
            }
        }
    }
}
