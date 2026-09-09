//! Private control commands terminate here, before ordinary storage or fanout.

use crate::{handlers::ingest::IngestError, state::AppState};
use buzz_core::{machine::MachineCommand, TenantContext};
use nostr::{Event, PublicKey};
use std::sync::Arc;

/// Validate the owner delegation and atomically persist a private command.
pub async fn handle(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    event: &Event,
) -> Result<bool, IngestError> {
    let invalid = |_| IngestError::Rejected("invalid: machine command or owner proof".into());
    let command = MachineCommand::from_event_after_signature(event).map_err(invalid)?;
    if let MachineCommand::Enroll(enrollment) = command {
        let signer = event.pubkey;
        let created_at = event.created_at.as_secs();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            buzz_core::machine::verify_enrollment_consent(
                &enrollment,
                &signer,
                created_at,
                chrono::Utc::now().timestamp(),
            )?;
            let coordinator = PublicKey::from_hex(&enrollment.coordinator_pubkey)
                .map_err(|_| "invalid coordinator")?;
            let proof =
                serde_json::to_string(&enrollment.owner_auth).map_err(|_| "invalid owner proof")?;
            let owner = buzz_sdk::nip_oa::verify_auth_tag_for_event(
                &proof,
                &coordinator,
                buzz_core::kind::KIND_MACHINE_ENROLLMENT,
                created_at,
            )
            .map_err(|_| "owner proof does not authorize enrollment")?;
            if owner != signer {
                return Err("enrollment signer does not own coordinator".into());
            }
            Ok(())
        })
        .await
        .map_err(|_| IngestError::Internal("machine proof verification worker failed".into()))?
        .map_err(|_| IngestError::Rejected("invalid: owner proof or coordinator consent".into()))?;
    }
    state
        .db
        .apply_machine_command(tenant.community(), event)
        .await
        .map_err(|error| match error {
            buzz_db::DbError::AccessDenied(_) | buzz_db::DbError::InvalidData(_) => {
                IngestError::Rejected("restricted: machine command unavailable".into())
            }
            buzz_db::DbError::Sqlx(sqlx::Error::Database(ref db_error))
                if db_error.code().as_deref() == Some("23505") =>
            {
                IngestError::Rejected(
                    "restricted: machine or coordinator already registered".into(),
                )
            }
            other => IngestError::Internal(format!("machine command persistence failed: {other}")),
        })
}
