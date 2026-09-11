use super::*;
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

fn receipt() -> FleetReceipt {
    FleetReceipt {
        attempt_id: format!("buzz-qualify-{}", "a".repeat(64)),
        task_id: Uuid::new_v4(),
        plan_event_id: "b".repeat(64),
        start_event_id: Some("c".repeat(64)),
        machine_id: "mack".into(),
        policy_digest: "d".repeat(64),
        status: ReceiptStatus::Success,
        qualification: Some(Qualification {
            repository: "mfethe1/buzz".into(),
            head_sha: "e".repeat(40),
            tracked_files: 7,
            python: "3.14".into(),
        }),
        error: None,
        completed_at: 100,
    }
}
fn event(body: &FleetReceipt, time: u64) -> Event {
    EventBuilder::new(
        Kind::Custom(KIND_JOB_RESULT as u16),
        body.to_canonical_json().unwrap(),
    )
    .tags([
        Tag::parse(["protocol", RECEIPT_PROTOCOL, "1"]).unwrap(),
        Tag::parse(["h", &Uuid::new_v4().to_string()]).unwrap(),
        Tag::parse(["d", &body.attempt_id]).unwrap(),
    ])
    .custom_created_at(Timestamp::from(time))
    .sign_with_keys(&Keys::generate())
    .unwrap()
}

#[test]
fn signed_receipt_keeps_completion_time_across_delayed_publication() {
    let body = receipt();
    let event = event(&body, 10_000);
    event.verify().unwrap();
    assert_eq!(
        FleetReceipt::from_event_after_signature(&event).unwrap().1,
        body
    );
    assert_ne!(event.created_at.as_secs(), body.completed_at);
    assert!(is_receipt(&event));
}

#[test]
fn receipt_rejects_future_completion_missing_start_and_noncanonical_content() {
    let body = receipt();
    assert!(FleetReceipt::from_event_after_signature(&event(&body, 99)).is_err());
    let mut absent = body.clone();
    absent.start_event_id = None;
    assert!(FleetReceipt::from_event_after_signature(&event(&absent, 100)).is_err());
    let original = event(&body, 100);
    let noncanonical = EventBuilder::new(original.kind, format!("{}\n", original.content))
        .tags(original.tags.clone())
        .custom_created_at(original.created_at)
        .sign_with_keys(&Keys::generate())
        .unwrap();
    assert!(FleetReceipt::from_event_after_signature(&noncanonical).is_err());
    let duplicate = EventBuilder::new(original.kind, original.content.clone())
        .tags(
            original
                .tags
                .iter()
                .cloned()
                .chain([Tag::parse(["d", &body.attempt_id]).unwrap()]),
        )
        .custom_created_at(original.created_at)
        .sign_with_keys(&Keys::generate())
        .unwrap();
    assert!(FleetReceipt::from_event_after_signature(&duplicate).is_err());
}

#[test]
fn unknown_is_not_success_or_proof_of_stopping() {
    let mut body = receipt();
    body.status = ReceiptStatus::Unknown;
    assert!(FleetReceipt::from_event_after_signature(&event(&body, 100)).is_err());
    body.qualification = None;
    assert_eq!(
        FleetReceipt::from_event_after_signature(&event(&body, 100))
            .unwrap()
            .1
            .status,
        ReceiptStatus::Unknown
    );
    body.status = ReceiptStatus::CancelledBeforeExecution;
    assert!(FleetReceipt::from_event_after_signature(&event(&body, 100)).is_err());
    body.start_event_id = None;
    assert_eq!(
        FleetReceipt::from_event_after_signature(&event(&body, 100))
            .unwrap()
            .1
            .status,
        ReceiptStatus::CancelledBeforeExecution
    );
}

#[test]
fn attempt_identity_binds_community_task_and_signed_plan() {
    let community = CommunityId::from_uuid(Uuid::new_v4());
    let task = Uuid::new_v4();
    let id = attempt_id(community, task, &[1; 32]);
    assert!(valid_attempt_id(&id));
    assert_eq!(id, attempt_id(community, task, &[1; 32]));
    assert_ne!(
        id,
        attempt_id(CommunityId::from_uuid(Uuid::new_v4()), task, &[1; 32])
    );
    assert_ne!(id, attempt_id(community, Uuid::new_v4(), &[1; 32]));
    assert_ne!(id, attempt_id(community, task, &[2; 32]));
}
