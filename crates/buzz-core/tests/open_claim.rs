//! Role-assignment transitions on open tasks (Fable critical #1 regression pins).

use buzz_core::cml::{
    CmlStatus, CmlTask, GitState, Presence, Priority, ReviewState, Roles, RuntimeState,
};
use buzz_core::cml_event::{reduce_cml_events, validate_cml_event, CmlRole, CmlTransition};
use buzz_core::kind::{KIND_JOB_ACCEPTED, KIND_JOB_REQUEST};
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
use uuid::Uuid;

const CHANNEL: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

fn hex(keys: &Keys) -> String {
    keys.public_key().to_hex()
}

fn open_task(planner: &Keys) -> CmlTask {
    CmlTask {
        acceptance: vec![],
        blockers: vec![],
        evidence: vec![],
        git: GitState {
            base_sha: "1".repeat(40),
            branch: "feat/open-claim".into(),
            head_sha: None,
            repo: "mfethe1/buzz".into(),
            worktree_alias: "open-claim".into(),
        },
        id: Uuid::parse_str("dc2bb0c1-d1ae-41a2-b315-7d7996c5ab93").unwrap(),
        lease: None,
        objective: "Open task claimable by any assigned-later worker".into(),
        priority: Priority::P1,
        protocol: "buzz-cml".into(),
        review: ReviewState {
            max_rounds: 3,
            round: 0,
        },
        roles: Roles {
            fixer: None,
            planner: hex(planner),
            reviewer: None,
            worker: None,
        },
        runtime: RuntimeState {
            host_id: None,
            last_heartbeat_at: None,
            presence: Presence::Offline,
            ttl_seconds: 180,
        },
        status: CmlStatus::Planned,
        title: "Open claim".into(),
        updated_at: 3_000,
        version: 1,
        extensions: Default::default(),
    }
}

fn claim_snapshot(plan: &CmlTask, worker_hex: &str, updated_at: u64) -> CmlTask {
    let mut claimed = plan.clone();
    claimed.status = CmlStatus::Claimed;
    claimed.updated_at = updated_at;
    claimed.roles.worker = Some(worker_hex.to_owned());
    claimed.lease = Some(buzz_core::cml::Lease {
        id: "lease-open".into(),
        holder: worker_hex.to_owned(),
        issued_at: updated_at,
        expires_at: updated_at + 3600,
    });
    claimed
}

fn signed(
    keys: &Keys,
    kind: u32,
    task: &CmlTask,
    transition: CmlTransition,
    role: CmlRole,
    prev: Option<&nostr::Event>,
) -> nostr::Event {
    let mut tags = vec![
        Tag::parse(["h", CHANNEL]).unwrap(),
        Tag::parse(["d", &task.id.to_string()]).unwrap(),
        Tag::parse(["protocol", "buzz-cml", "1"]).unwrap(),
        Tag::parse(["transition", transition.as_str()]).unwrap(),
        Tag::parse(["status", status_name(task.status)]).unwrap(),
        Tag::parse(["role", role.as_str()]).unwrap(),
    ];
    if let Some(p) = prev {
        tags.push(Tag::parse(["e", &p.id.to_hex(), "prev"]).unwrap());
    }
    EventBuilder::new(Kind::Custom(kind as u16), task.to_canonical_json().unwrap())
        .tags(tags)
        .custom_created_at(Timestamp::from(task.updated_at))
        .sign_with_keys(keys)
        .unwrap()
}

fn status_name(status: CmlStatus) -> &'static str {
    match status {
        CmlStatus::Planned => "planned",
        CmlStatus::Claimed => "claimed",
        CmlStatus::Working => "working",
        _ => "planned",
    }
}

#[test]
fn open_task_can_be_claimed_by_a_worker_assigning_the_empty_role() {
    let planner = Keys::generate();
    let worker = Keys::generate();
    let planned = open_task(&planner);
    let plan = signed(
        &planner,
        KIND_JOB_REQUEST,
        &planned,
        CmlTransition::Plan,
        CmlRole::Planner,
        None,
    );

    // The worker's claim fills the previously-null roles.worker slot.
    let claimed = claim_snapshot(&planned, &hex(&worker), 3_010);
    let claim = signed(
        &worker,
        KIND_JOB_ACCEPTED,
        &claimed,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(&plan),
    );

    assert!(
        validate_cml_event(&claim).is_ok(),
        "claim on an open task must validate"
    );
    let reduced = reduce_cml_events(&[plan, claim]).expect("open claim must reduce");
    assert_eq!(reduced.task.status, CmlStatus::Claimed);
    assert_eq!(
        reduced.task.roles.worker.as_deref(),
        Some(hex(&worker).as_str())
    );
}

#[test]
fn role_reassignment_after_assignment_is_still_forgery() {
    let planner = Keys::generate();
    let worker = Keys::generate();
    let impostor = Keys::generate();
    let planned = open_task(&planner);
    let plan = signed(
        &planner,
        KIND_JOB_REQUEST,
        &planned,
        CmlTransition::Plan,
        CmlRole::Planner,
        None,
    );

    // Once worker holds the role, an impostor may not rewrite roles.worker.
    let claimed = claim_snapshot(&planned, &hex(&worker), 3_010);
    let claim = signed(
        &worker,
        KIND_JOB_ACCEPTED,
        &claimed,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(&plan),
    );

    let mut stolen = claim_snapshot(&planned, &hex(&impostor), 3_011);
    stolen.roles.worker = Some(hex(&impostor));
    let steal = signed(
        &impostor,
        KIND_JOB_ACCEPTED,
        &stolen,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(&plan),
    );

    // Impostor's event is self-consistent (it names itself) but rewrites an
    // already-assigned role: the reducer must surface the fork, never accept it.
    let reduced = reduce_cml_events(&[plan, claim, steal]).expect("fork reduces");
    assert!(reduced.conflicted);
}

#[test]
fn forged_sibling_cannot_hold_a_claimed_task_hostage() {
    let planner = Keys::generate();
    let worker = Keys::generate();
    let griefer = Keys::generate();
    let planned = open_task(&planner);
    let plan = signed(
        &planner,
        KIND_JOB_REQUEST,
        &planned,
        CmlTransition::Plan,
        CmlRole::Planner,
        None,
    );

    let claimed = claim_snapshot(&planned, &hex(&worker), 3_010);
    let claim = signed(
        &worker,
        KIND_JOB_ACCEPTED,
        &claimed,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(&plan),
    );

    // A griefer publishes a second claim for the same predecessor, naming
    // itself as worker. Under the plan's null-worker contract the griefer is
    // not yet excluded at single-event validation, but the impostor's
    // snapshot rewrites an assigned role (worker) — after the fix the fork
    // branch checks each sibling's legality and quarantines the forgery
    // instead of forcing the whole task into conflicted.
    let start_snapshot = {
        let mut s = claimed.clone();
        s.status = CmlStatus::Working;
        s.updated_at = 3_020;
        s
    };
    let start = signed(
        &worker,
        buzz_core::kind::KIND_JOB_PROGRESS,
        &start_snapshot,
        CmlTransition::Start,
        CmlRole::Worker,
        Some(&claim),
    );
    let mut grief_snapshot = claim_snapshot(&planned, &hex(&griefer), 3_015);
    grief_snapshot.roles.worker = Some(hex(&griefer));
    let grief = signed(
        &griefer,
        KIND_JOB_ACCEPTED,
        &grief_snapshot,
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(&claim),
    );

    // The legal chain (plan->claim->start) plus the forged sibling of claim:
    // reduction must continue along the legal chain, not surface conflicted.
    let reduced = reduce_cml_events(&[plan.clone(), claim.clone(), grief, start])
        .expect("reduce with griefer");
    assert!(
        !reduced.conflicted,
        "a forged sibling must not hold the task hostage"
    );
    assert_eq!(reduced.task.status, CmlStatus::Working);
    // A genuine fork (two legal claims racing an open plan) still conflicts.
    let rival = signed(
        &griefer,
        KIND_JOB_ACCEPTED,
        &claim_snapshot(&planned, &hex(&griefer), 3_012),
        CmlTransition::Claim,
        CmlRole::Worker,
        Some(&plan),
    );
    let raced = reduce_cml_events(&[plan, claim, rival]).unwrap();
    assert!(
        raced.conflicted,
        "two legal claims on an open plan still conflict"
    );
}
