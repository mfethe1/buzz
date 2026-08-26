//! Duplicate-claim rejection: two workers race for one plan, exactly one wins.
//!
//! The lease model guarantees an exclusive claim. This test pins that two
//! sig-valid `43002` accepts for the same predecessor cannot both reduce to
//! an active claim: the reducer must surface `conflicted` and only an
//! authorized `owner.resolve` selecting one head recovers a single worker.

use buzz_core::cml::{
    CmlStatus, CmlTask, GitState, Presence, Priority, ReviewState, Roles, RuntimeState,
};
use buzz_core::cml_event::{reduce_cml_events, validate_cml_event, CmlRole, CmlTransition};
use buzz_core::kind::{KIND_JOB_ACCEPTED, KIND_JOB_PROGRESS, KIND_JOB_REQUEST};
use nostr::{Event, EventBuilder, EventId, Keys, Kind, Tag, Timestamp};
use uuid::Uuid;

const CHANNEL: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

fn hex(keys: &Keys) -> String {
    keys.public_key().to_hex()
}

fn snapshot(
    status: CmlStatus,
    roles: &Roles,
    lease_holder: Option<&str>,
    updated_at: u64,
) -> CmlTask {
    CmlTask {
        acceptance: vec![],
        blockers: vec![],
        evidence: vec![],
        git: GitState {
            base_sha: "1".repeat(40),
            branch: "feat/dup-claim".into(),
            head_sha: None,
            repo: "mfethe1/buzz".into(),
            worktree_alias: "dup-claim".into(),
        },
        id: Uuid::parse_str("dc2bb0c1-d1ae-41a2-b315-7d7996c5ab93").unwrap(),
        lease: lease_holder.map(|holder| buzz_core::cml::Lease {
            id: "lease-dup".into(),
            holder: holder.to_owned(),
            issued_at: updated_at,
            expires_at: updated_at + 3600,
        }),
        objective: "Duplicate claim rejection".into(),
        priority: Priority::P1,
        protocol: "buzz-cml".into(),
        review: ReviewState {
            max_rounds: 3,
            round: 0,
        },
        roles: roles.clone(),
        runtime: RuntimeState {
            host_id: None,
            last_heartbeat_at: None,
            presence: Presence::Offline,
            ttl_seconds: 180,
        },
        status,
        title: "Dup claim".into(),
        updated_at,
        version: 1,
        extensions: Default::default(),
    }
}

fn signed(
    keys: &Keys,
    kind: u32,
    task: &CmlTask,
    transition: CmlTransition,
    role: CmlRole,
    previous: Option<EventId>,
) -> Event {
    let mut tags = vec![
        Tag::parse(["h", CHANNEL]).unwrap(),
        Tag::parse(["d", &task.id.to_string()]).unwrap(),
        Tag::parse(["protocol", "buzz-cml", "1"]).unwrap(),
        Tag::parse(["transition", transition.as_str()]).unwrap(),
        Tag::parse(["status", status_name(task.status)]).unwrap(),
        Tag::parse(["role", role.as_str()]).unwrap(),
    ];
    if let Some(prev) = previous {
        tags.push(Tag::parse(["e", &prev.to_hex(), "prev"]).unwrap());
    }
    EventBuilder::new(Kind::Custom(kind as u16), task.to_canonical_json().unwrap())
        .tags(tags)
        .custom_created_at(Timestamp::from(task.updated_at))
        .sign_with_keys(keys)
        .unwrap()
}

fn resolution(planner: &Keys, task: &CmlTask, a: EventId, b: EventId, selected: EventId) -> Event {
    let tags = vec![
        Tag::parse(["h", CHANNEL]).unwrap(),
        Tag::parse(["d", &task.id.to_string()]).unwrap(),
        Tag::parse(["protocol", "buzz-cml", "1"]).unwrap(),
        Tag::parse(["transition", "owner.resolve"]).unwrap(),
        Tag::parse(["status", status_name(task.status)]).unwrap(),
        Tag::parse(["role", "planner"]).unwrap(),
        Tag::parse(["e", &a.to_hex(), "fork_a"]).unwrap(),
        Tag::parse(["e", &b.to_hex(), "fork_b"]).unwrap(),
        Tag::parse(["e", &selected.to_hex(), "selected"]).unwrap(),
    ];
    EventBuilder::new(
        Kind::Custom(KIND_JOB_PROGRESS as u16),
        task.to_canonical_json().unwrap(),
    )
    .tags(tags)
    .custom_created_at(Timestamp::from(task.updated_at))
    .sign_with_keys(planner)
    .unwrap()
}

fn status_name(status: CmlStatus) -> &'static str {
    match status {
        CmlStatus::Claimed => "claimed",
        _ => "planned",
    }
}

#[test]
fn two_distinct_workers_claiming_one_plan_reduce_to_conflicted_not_two_winners() {
    let planner = Keys::generate();
    let worker_a = Keys::generate();
    let worker_b = Keys::generate();
    // Both workers are technically assignable, but leases are exclusive.
    let roles = Roles {
        fixer: None,
        planner: hex(&planner),
        reviewer: None,
        worker: Some(hex(&worker_a)),
    };

    let planned = snapshot(CmlStatus::Planned, &roles, None, 1_000);
    let plan = signed(
        &planner,
        KIND_JOB_REQUEST,
        &planned,
        CmlTransition::Plan,
        CmlRole::Planner,
        None,
    );

    let claim_a_task = snapshot(CmlStatus::Claimed, &roles, Some(&hex(&worker_a)), 1_010);
    let claim_a = signed(
        &worker_a,
        KIND_JOB_ACCEPTED,
        &claim_a_task,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(plan.id),
    );

    // worker_b signs a claim whose snapshot rewrites roles.worker = worker_b.
    // In isolation that event is self-consistent (single-event validation
    // cannot see the plan), so it validates on its own — the guard is
    // cross-event: contract immutability in the reducer.
    let mut roles_b = roles.clone();
    roles_b.worker = Some(hex(&worker_b));
    let claim_b_task = snapshot(CmlStatus::Claimed, &roles_b, Some(&hex(&worker_b)), 1_011);
    let claim_b = signed(
        &worker_b,
        KIND_JOB_ACCEPTED,
        &claim_b_task,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(plan.id),
    );

    // plan + claim_a reduce to a single active claim.
    let reduced = reduce_cml_events(&[plan.clone(), claim_a.clone()]).unwrap();
    assert_eq!(reduced.task.status, CmlStatus::Claimed);
    assert!(!reduced.conflicted);

    // The duplicate is valid as a standalone event but can never win or even
    // disturb this task's chain: the plan pre-assigned worker_a, so worker_b's
    // claim rewrites an assigned role. After the forged-sibling fix the
    // reducer quarantines it — the task stays claimed by worker_a instead of
    // being held hostage in `conflicted`.
    assert!(validate_cml_event(&claim_b).is_ok());
    let reduced = reduce_cml_events(&[plan, claim_a, claim_b]).unwrap();
    assert!(!reduced.conflicted, "role-theft is quarantined, not a fork");
    assert_eq!(reduced.task.status, CmlStatus::Claimed);
    assert_eq!(
        reduced.task.roles.worker.as_deref(),
        Some(hex(&worker_a).as_str()),
        "the assigned worker keeps the claim"
    );
}

#[test]
fn two_claims_from_the_same_assigned_worker_fork_into_conflicted() {
    let planner = Keys::generate();
    let worker = Keys::generate();
    let roles = Roles {
        fixer: None,
        planner: hex(&planner),
        reviewer: None,
        worker: Some(hex(&worker)),
    };

    let planned = snapshot(CmlStatus::Planned, &roles, None, 2_000);
    let plan = signed(
        &planner,
        KIND_JOB_REQUEST,
        &planned,
        CmlTransition::Plan,
        CmlRole::Planner,
        None,
    );

    let first_task = snapshot(CmlStatus::Claimed, &roles, Some(&hex(&worker)), 2_010);
    let first = signed(
        &worker,
        KIND_JOB_ACCEPTED,
        &first_task,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(plan.id),
    );
    // Distinct updated_at -> distinct event id, same predecessor: a replay fork.
    let mut second_task = snapshot(CmlStatus::Claimed, &roles, Some(&hex(&worker)), 2_011);
    second_task.lease = first_task.lease.clone();
    let second = signed(
        &worker,
        KIND_JOB_ACCEPTED,
        &second_task,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(plan.id),
    );

    let reduced = reduce_cml_events(&[plan.clone(), second.clone(), first.clone()]).unwrap();
    assert!(
        reduced.conflicted,
        "sibling claims must not silently pick a winner"
    );
    assert_eq!(reduced.task.status, CmlStatus::Conflicted);

    // owner.resolve selects exactly one head and recovers a single claim.
    // The resolution snapshot must be the selected head's snapshot with only
    // updated_at advanced.
    let mut resolution_task = first_task.clone();
    resolution_task.updated_at = 2_021;
    let resolve = resolution(&planner, &resolution_task, first.id, second.id, first.id);
    let recovered = reduce_cml_events(&[plan, second, first, resolve]).unwrap();
    assert!(!recovered.conflicted);
    assert_eq!(recovered.task.status, CmlStatus::Claimed);
    assert_eq!(
        recovered.task.lease.as_ref().map(|l| l.holder.as_str()),
        Some(hex(&worker).as_str())
    );
}
