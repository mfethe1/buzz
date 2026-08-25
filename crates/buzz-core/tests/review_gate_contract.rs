//! Blind review-gate contract tests (S6 core).

use buzz_core::cml_event::{CmlTransition, ReducedCmlTask};
use buzz_core::review_gate::{enforce_round_bound, verify_blind, GateVerdict, ReviewGateInput};
use uuid::Uuid;

fn reduced_task(task_id: Uuid) -> ReducedCmlTask {
    ReducedCmlTask {
        head: nostr::EventId::from_slice(&[0u8; 32]).expect("test event id"),
        conflicted: false,
        task: buzz_core::cml::parse_cml(&minimal_cml(task_id)).expect("valid task"),
    }
}

fn minimal_cml(task_id: Uuid) -> String {
    format!(
        r#"{{
  "acceptance": [],
  "blockers": [],
  "evidence": [],
  "git": {{"base_sha": "{}", "branch": "feat/gate", "head_sha": null, "repo": "block/buzz", "worktree_alias": "gate"}},
  "id": "{task_id}",
  "lease": null,
  "objective": "Gate contract",
  "priority": "P1",
  "protocol": "buzz-cml",
  "review": {{"max_rounds": 3, "round": 0}},
  "roles": {{"fixer": null, "planner": "{}", "reviewer": null, "worker": "{}"}},
  "runtime": {{"host_id": null, "last_heartbeat_at": null, "presence": "offline", "ttl_seconds": 180}},
  "status": "verified",
  "title": "Gate",
  "updated_at": 1787673000,
  "version": 1
}}"#,
        "1".repeat(40),
        "a".repeat(64),
        "b".repeat(64),
    )
}

fn input_for(task_id: Uuid, verified: Vec<bool>, evidence: Vec<String>) -> ReviewGateInput {
    ReviewGateInput {
        task_id,
        channel_id: Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap(),
        expected_acceptance: verified
            .into_iter()
            .enumerate()
            .map(|(i, v)| (format!("A{}", i + 1), format!("criterion {i}"), v))
            .collect(),
        evidence_hashes: evidence,
    }
}

#[test]
fn mismatched_task_id_is_a_hard_error() {
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    assert!(verify_blind(
        &input_for(a, vec![true], vec!["h".into()]),
        &reduced_task(b)
    )
    .is_err());
}

#[test]
fn empty_criteria_is_insufficient_never_approved() {
    let id = Uuid::new_v4();
    let verdict =
        verify_blind(&input_for(id, vec![], vec!["h".into()]), &reduced_task(id)).unwrap();
    assert!(matches!(verdict, GateVerdict::InsufficientEvidence(_)));
}

#[test]
fn all_verified_with_evidence_approves() {
    let id = Uuid::new_v4();
    let verdict = verify_blind(
        &input_for(id, vec![true, true], vec!["abc".into()]),
        &reduced_task(id),
    )
    .unwrap();
    assert_eq!(verdict, GateVerdict::Approved);
}

#[test]
fn unverified_criterion_rejects_with_exactly_one_finding_each() {
    let id = Uuid::new_v4();
    let verdict = verify_blind(
        &input_for(id, vec![true, false, false], vec!["abc".into()]),
        &reduced_task(id),
    )
    .unwrap();
    match verdict {
        GateVerdict::Rejected(findings) => {
            assert_eq!(findings.len(), 2);
            assert_eq!(findings[0].criterion_id, "A2");
            assert_eq!(findings[1].criterion_id, "A3");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn verified_claims_without_evidence_are_insufficient() {
    let id = Uuid::new_v4();
    let verdict = verify_blind(&input_for(id, vec![true], vec![]), &reduced_task(id)).unwrap();
    assert!(matches!(verdict, GateVerdict::InsufficientEvidence(missing) if missing == vec!["A1"]));
}

#[test]
fn missing_evidence_outranks_findings() {
    let id = Uuid::new_v4();
    // A2 fails AND A1 has no evidence: Insufficient wins deterministically.
    let verdict =
        verify_blind(&input_for(id, vec![true, false], vec![]), &reduced_task(id)).unwrap();
    assert!(matches!(verdict, GateVerdict::InsufficientEvidence(_)));
}

#[test]
fn reject_increments_round_by_exactly_one() {
    assert_eq!(
        enforce_round_bound(0, CmlTransition::ReviewReject).unwrap(),
        1
    );
    assert_eq!(
        enforce_round_bound(1, CmlTransition::ReviewReject).unwrap(),
        2
    );
    assert_eq!(
        enforce_round_bound(2, CmlTransition::ReviewReject).unwrap(),
        3
    );
}

#[test]
fn fourth_reject_is_impossible() {
    assert!(enforce_round_bound(3, CmlTransition::ReviewReject).is_err());
}

#[test]
fn non_review_transitions_are_invalid_here() {
    for t in [
        CmlTransition::Plan,
        CmlTransition::Claim,
        CmlTransition::Merge,
    ] {
        assert!(enforce_round_bound(0, t).is_err());
    }
}

#[test]
fn submit_fixsubmit_and_approve_are_round_neutral() {
    for t in [
        CmlTransition::Submit,
        CmlTransition::FixSubmit,
        CmlTransition::ReviewApprove,
    ] {
        assert_eq!(enforce_round_bound(2, t).unwrap(), 2);
    }
}
