//! Private signed machine journal and immutable enrollment projection.

use buzz_core::{machine::*, CommunityId};
use buzz_datastore_tracing::datastore_span;
use nostr::Event;
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    observability::{acquire_writer, WriterOperation},
    Db, DbError, Result,
};

fn denied() -> DbError {
    DbError::AccessDenied("machine registration or coordinator unavailable".into())
}

impl Db {
    /// Persist a verified command and its projection in one primary transaction.
    /// The shared ingest handler must verify the event, owner proof, transport
    /// identity, scope and moderation before this private-store seam is reached.
    #[datastore_span(name = "apply_machine_command", system = "postgresql")]
    pub async fn apply_machine_command(
        &self,
        community: CommunityId,
        event: &Event,
    ) -> Result<bool> {
        let command =
            MachineCommand::from_event_after_signature(event).map_err(DbError::InvalidData)?;
        if command.community_id() != *community.as_uuid() {
            return Err(denied());
        }
        let connection = acquire_writer(&self.pool, WriterOperation::EventWrite).await?;
        let mut tx = Transaction::begin(connection, None).await?;
        let active: bool = sqlx::query_scalar("SELECT community_write_allowed($1)")
            .bind(community.as_uuid())
            .fetch_one(&mut *tx)
            .await?;
        if !active {
            return Err(denied());
        }
        // Serialize absent-row enrollment and duplicate admission for the same
        // tenant/machine. Collisions only serialize; they never grant access.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!(
                "machine:{}:{}",
                community.as_uuid(),
                command.machine_id()
            ))
            .execute(&mut *tx)
            .await?;
        let duplicate: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM machine_control_events WHERE community_id=$1 AND event_id=$2)")
            .bind(community.as_uuid()).bind(event.id.as_bytes().as_slice()).fetch_one(&mut *tx).await?;
        if duplicate {
            return Ok(false);
        }
        let owner = match &command {
            MachineCommand::Enroll(enrollment) => {
                enroll(&mut tx, community, event, enrollment).await?
            }
            MachineCommand::Observe(observation) => {
                observe(&mut tx, community, event, observation).await?
            }
        };
        sqlx::query("INSERT INTO machine_control_events(community_id,event_id,machine_id,owner_pubkey,kind,signed_event) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(community.as_uuid()).bind(event.id.as_bytes().as_slice()).bind(command.machine_id())
            .bind(owner).bind(i32::from(event.kind.as_u16())).bind(serde_json::to_value(event)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Read only the authenticated owner's machines, filtering before pagination.
    /// Always reads the primary; observations are freshness-bounded display data.
    #[datastore_span(name = "list_machines", system = "postgresql")]
    pub async fn list_machines(
        &self,
        community: CommunityId,
        owner: &[u8],
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<Value>> {
        if !(1..=101).contains(&limit) {
            return Err(DbError::InvalidData("invalid machine limit".into()));
        }
        let rows = sqlx::query("SELECT m.*, e.signed_event AS enrollment, o.signed_event AS observation, (m.expires_at > clock_timestamp() AND EXISTS(SELECT 1 FROM users u JOIN users owner ON owner.community_id=u.community_id AND owner.pubkey=m.owner_pubkey WHERE u.community_id=m.community_id AND u.pubkey=m.coordinator_pubkey AND u.agent_owner_pubkey=m.owner_pubkey AND u.machine_id=m.machine_id::text AND u.agent_type=m.runtime AND u.deactivated_at IS NULL AND owner.deactivated_at IS NULL)) AS fresh FROM machines m JOIN machine_control_events e ON e.community_id=m.community_id AND e.event_id=m.registration_event_id LEFT JOIN machine_control_events o ON o.community_id=m.community_id AND o.event_id=m.observation_event_id WHERE m.community_id=$1 AND m.owner_pubkey=$2 AND ($3::uuid IS NULL OR m.machine_id>$3) ORDER BY m.machine_id LIMIT $4")
            .bind(community.as_uuid()).bind(owner).bind(after).bind(limit).fetch_all(&self.pool).await?;
        rows.iter().map(machine_json).collect()
    }

    /// Uniform absent result for missing, other-owner and other-tenant machines.
    #[datastore_span(name = "get_machine", system = "postgresql")]
    pub async fn get_machine(
        &self,
        community: CommunityId,
        owner: &[u8],
        machine: Uuid,
    ) -> Result<Option<Value>> {
        let row = sqlx::query("SELECT m.*, e.signed_event AS enrollment, o.signed_event AS observation, (m.expires_at > clock_timestamp() AND EXISTS(SELECT 1 FROM users u JOIN users owner ON owner.community_id=u.community_id AND owner.pubkey=m.owner_pubkey WHERE u.community_id=m.community_id AND u.pubkey=m.coordinator_pubkey AND u.agent_owner_pubkey=m.owner_pubkey AND u.machine_id=m.machine_id::text AND u.agent_type=m.runtime AND u.deactivated_at IS NULL AND owner.deactivated_at IS NULL)) AS fresh FROM machines m JOIN machine_control_events e ON e.community_id=m.community_id AND e.event_id=m.registration_event_id LEFT JOIN machine_control_events o ON o.community_id=m.community_id AND o.event_id=m.observation_event_id WHERE m.community_id=$1 AND m.owner_pubkey=$2 AND m.machine_id=$3")
            .bind(community.as_uuid()).bind(owner).bind(machine).fetch_optional(&self.pool).await?;
        row.as_ref().map(machine_json).transpose()
    }
}

fn machine_json(row: &sqlx::postgres::PgRow) -> Result<Value> {
    Ok(json!({
        "machine_id": row.try_get::<Uuid,_>("machine_id")?,
        "owner_pubkey": hex::encode(row.try_get::<Vec<u8>,_>("owner_pubkey")?),
        "coordinator_pubkey": hex::encode(row.try_get::<Vec<u8>,_>("coordinator_pubkey")?),
        "label": row.try_get::<String,_>("label")?,
        "runtime": row.try_get::<String,_>("runtime")?,
        "registration_event_id": hex::encode(row.try_get::<Vec<u8>,_>("registration_event_id")?),
        "observation_sequence": row.try_get::<i64,_>("observation_sequence")?,
        "reported_state": row.try_get::<Option<String>,_>("observed_state")?,
        "fresh": row.try_get::<Option<bool>,_>("fresh")?.unwrap_or(false),
        "observed_at": row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("observed_at")?,
        "received_at": row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("received_at")?,
        "expires_at": row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>("expires_at")?,
        "enrollment_event": row.try_get::<Value,_>("enrollment")?,
        "observation_event": row.try_get::<Option<Value>,_>("observation")?,
    }))
}

async fn enroll(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    event: &Event,
    enrollment: &MachineEnrollment,
) -> Result<Vec<u8>> {
    let owner = event.pubkey.to_bytes().to_vec();
    let coordinator = hex::decode(&enrollment.coordinator_pubkey).map_err(|_| denied())?;
    let existing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM machines WHERE community_id=$1 AND (machine_id=$2 OR coordinator_pubkey=$3))")
        .bind(community.as_uuid()).bind(enrollment.machine_id).bind(&coordinator).fetch_one(&mut **tx).await?;
    if existing {
        return Err(denied());
    }
    // Deterministic user-row order also serializes different machine claims for
    // one coordinator. No pre-transaction owner or profile materialization.
    let mut identities = [owner.clone(), coordinator.clone()];
    identities.sort();
    for key in identities {
        sqlx::query("INSERT INTO users(community_id,pubkey) VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(community.as_uuid())
            .bind(&key)
            .execute(&mut **tx)
            .await?;
        let active: bool = sqlx::query_scalar("SELECT deactivated_at IS NULL FROM users WHERE community_id=$1 AND pubkey=$2 FOR UPDATE")
            .bind(community.as_uuid()).bind(&key).fetch_one(&mut **tx).await?;
        if !active {
            return Err(denied());
        }
    }
    let row = sqlx::query("SELECT agent_owner_pubkey, agent_type, machine_id FROM users WHERE community_id=$1 AND pubkey=$2")
        .bind(community.as_uuid()).bind(&coordinator).fetch_one(&mut **tx).await?;
    if row
        .try_get::<Option<Vec<u8>>, _>("agent_owner_pubkey")?
        .is_some_and(|v| v != owner)
        || row.try_get::<Option<String>, _>("machine_id")?.is_some()
        || row
            .try_get::<Option<String>, _>("agent_type")?
            .is_some_and(|v| v != enrollment.runtime.as_str())
    {
        return Err(denied());
    }
    let now: i64 =
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint")
            .fetch_one(&mut **tx)
            .await?;
    validate_enrollment_consent_after_signature(
        enrollment,
        &event.pubkey,
        event.created_at.as_secs(),
        now,
    )
    .map_err(|_| denied())?;
    sqlx::query("UPDATE users SET agent_owner_pubkey=$3,agent_type=$4,machine_id=$5,machine_label=$6,machine_runtime=$4,updated_at=clock_timestamp() WHERE community_id=$1 AND pubkey=$2")
        .bind(community.as_uuid()).bind(&coordinator).bind(&owner).bind(enrollment.runtime.as_str()).bind(enrollment.machine_id.to_string()).bind(&enrollment.label).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO machines(community_id,machine_id,owner_pubkey,coordinator_pubkey,registration_event_id,label,runtime) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(community.as_uuid()).bind(enrollment.machine_id).bind(&owner).bind(coordinator).bind(event.id.as_bytes().as_slice()).bind(&enrollment.label).bind(enrollment.runtime.as_str()).execute(&mut **tx).await?;
    Ok(owner)
}

async fn observe(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    event: &Event,
    observation: &MachineObservation,
) -> Result<Vec<u8>> {
    let row = sqlx::query("SELECT m.* FROM machines m JOIN users u ON u.community_id=m.community_id AND u.pubkey=m.coordinator_pubkey JOIN users owner ON owner.community_id=m.community_id AND owner.pubkey=m.owner_pubkey WHERE m.community_id=$1 AND m.machine_id=$2 AND m.coordinator_pubkey=$3 AND u.agent_owner_pubkey=m.owner_pubkey AND u.machine_id=m.machine_id::text AND u.agent_type=m.runtime AND u.deactivated_at IS NULL AND owner.deactivated_at IS NULL FOR UPDATE OF m FOR SHARE OF u, owner")
        .bind(community.as_uuid()).bind(observation.machine_id).bind(event.pubkey.to_bytes().as_slice()).fetch_optional(&mut **tx).await?.ok_or_else(denied)?;
    if hex::encode(row.try_get::<Vec<u8>, _>("registration_event_id")?)
        != observation.registration_event_id
        || row.try_get::<i64, _>("observation_sequence")? >= observation.sequence
    {
        return Err(denied());
    }
    let received: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    let signed = i64::try_from(event.created_at.as_secs()).map_err(|_| denied())?;
    if signed < received.timestamp() - MAX_OBSERVATION_AGE_SECS
        || signed > received.timestamp() + MAX_CLOCK_SKEW_SECS
    {
        return Err(denied());
    }
    let observed_at = chrono::DateTime::from_timestamp(signed, 0).ok_or_else(denied)?;
    let expires = received.min(observed_at) + chrono::Duration::seconds(OBSERVATION_TTL_SECS);
    let state = match observation.state {
        MachineState::Ready => "ready",
        MachineState::Busy => "busy",
        MachineState::Unavailable => "unavailable",
    };
    sqlx::query("UPDATE machines SET observation_event_id=$3,observation_sequence=$4,observed_state=$5,observed_at=$6,received_at=$7,expires_at=$8 WHERE community_id=$1 AND machine_id=$2")
        .bind(community.as_uuid()).bind(observation.machine_id).bind(event.id.as_bytes().as_slice()).bind(observation.sequence).bind(state).bind(observed_at).bind(received).bind(expires).execute(&mut **tx).await?;
    row.try_get("owner_pubkey").map_err(Into::into)
}

#[cfg(test)]
mod postgres_tests;
