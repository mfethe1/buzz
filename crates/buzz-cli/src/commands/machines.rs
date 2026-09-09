//! Signed private enrollment and observation, and owner-only machine reads.
use crate::{client::BuzzClient, error::CliError, validate::read_file_or_stdin, MachinesCmd};
use buzz_core::machine::{
    verify_enrollment_consent, MachineCommand, MachineEnrollment, MachineEnrollmentConsent,
};
use nostr::{EventBuilder, Kind, PublicKey, Timestamp};
use uuid::Uuid;

/// Execute one machine operation using the existing authenticated event transport.
pub async fn dispatch(command: MachinesCmd, client: &BuzzClient) -> Result<(), CliError> {
    let response = match command {
        MachinesCmd::Authorize { coordinator } => {
            let coordinator = PublicKey::from_hex(&coordinator)
                .map_err(|_| CliError::Usage("coordinator must be a public key".into()))?;
            buzz_sdk::nip_oa::compute_auth_tag(
                client.keys(),
                &coordinator,
                &format!("kind=47210&created_at<{}", Timestamp::now().as_secs() + 300),
            )
            .map_err(crate::validate::sdk_err)?
        }
        MachinesCmd::Consent { file } => consent(client, &file)?,
        MachinesCmd::List { after, limit } => {
            if !(1..=100).contains(&limit) {
                return Err(CliError::Usage("limit must be between 1 and 100".into()));
            }
            let mut path = format!("/api/machines?limit={limit}");
            if let Some(after) = after {
                path.push_str(&format!("&after={after}"));
            }
            client.get_authed(&path).await?
        }
        MachinesCmd::Get { id } => client.get_authed(&format!("/api/machines/{id}")).await?,
        MachinesCmd::Enroll { file } => {
            publish(client, &file, buzz_core::kind::KIND_MACHINE_ENROLLMENT).await?
        }
        MachinesCmd::Observe { file } => {
            publish(client, &file, buzz_core::kind::KIND_MACHINE_OBSERVATION).await?
        }
    };
    println!("{response}");
    Ok(())
}

async fn publish(client: &BuzzClient, file: &str, kind: u32) -> Result<String, CliError> {
    let content = read_file_or_stdin(file)?;
    let event = EventBuilder::new(Kind::Custom(kind as u16), content)
        .sign_with_keys(client.keys())
        .map_err(|_| CliError::Other("machine command signing failed".into()))?;
    MachineCommand::from_event_after_signature(&event).map_err(CliError::Usage)?;
    client.submit_event(event).await
}

/// Parse a machine identifier without accepting hostnames or connection strings.
pub fn parse_id(value: &str) -> Result<Uuid, String> {
    Uuid::parse_str(value).map_err(|_| "machine ID must be a UUID".into())
}

fn consent(client: &BuzzClient, file: &str) -> Result<String, CliError> {
    let content = read_file_or_stdin(file)?;
    if content.len() > 2048 {
        return Err(CliError::Usage("consent exceeds 2048 bytes".into()));
    }
    let consent: MachineEnrollmentConsent = serde_json::from_str(&content)
        .map_err(|_| CliError::Usage("invalid consent document".into()))?;
    let owner = PublicKey::from_hex(&consent.owner_pubkey)
        .map_err(|_| CliError::Usage("invalid consent owner".into()))?;
    let event = EventBuilder::new(Kind::Custom(47212), content)
        .sign_with_keys(client.keys())
        .map_err(|_| CliError::Other("consent signing failed".into()))?;
    let enrollment = MachineEnrollment {
        version: consent.version,
        community_id: consent.community_id,
        machine_id: consent.machine_id,
        coordinator_pubkey: consent.coordinator_pubkey,
        label: consent.label,
        runtime: consent.runtime,
        owner_auth: consent.owner_auth,
        coordinator_consent: event.clone(),
    };
    verify_enrollment_consent(
        &enrollment,
        &owner,
        event.created_at.as_secs(),
        Timestamp::now().as_secs() as i64,
    )
    .map_err(CliError::Usage)?;
    let proof = serde_json::to_string(&enrollment.owner_auth)
        .map_err(|_| CliError::Usage("invalid owner proof".into()))?;
    let proved = buzz_sdk::nip_oa::verify_auth_tag_for_event(
        &proof,
        &client.keys().public_key(),
        47210,
        event.created_at.as_secs(),
    )
    .map_err(crate::validate::sdk_err)?;
    if proved != owner {
        return Err(CliError::Usage("consent owner does not match proof".into()));
    }
    serde_json::to_string(&event)
        .map_err(|_| CliError::Other("consent serialization failed".into()))
}
