use super::fixtures::Fixture;
use super::*;
use buzz_core::cml::CmlStatus;

#[tokio::test]
#[ignore = "requires Postgres"]
async fn submit_binds_observed_commit_and_exact_signed_receipt() {
    let f = Fixture::new().await;
    let (plan, start, mut task) = f.start().await;
    let receipt = f.receipt(&plan, Some(&start), ReceiptStatus::Success);
    f.persist(&receipt).await.unwrap();
    task.status = CmlStatus::Review;
    task.git.head_sha = Some("c".repeat(40));
    task.evidence.push(buzz_core::cml::Evidence {
        kind: "fleet-qualification-receipt".into(),
        reference: receipt.id.to_hex(),
    });
    for case in [
        "wrong_commit",
        "missing_receipt",
        "wrong_receipt",
        "wrong_kind",
    ] {
        let mut invalid = task.clone();
        match case {
            "wrong_commit" => invalid.git.head_sha = Some("d".repeat(40)),
            "missing_receipt" => invalid.evidence.clear(),
            "wrong_receipt" => invalid.evidence[0].reference = "e".repeat(64),
            "wrong_kind" => invalid.evidence[0].kind = "test".into(),
            _ => unreachable!(),
        }
        let event = f.event(&invalid, CmlTransition::Submit, Some(&start));
        assert!(f.persist(&event).await.is_err(), "accepted {case}");
        assert_eq!(f.event_count(&event).await, 0, "stored {case}");
        let head: Vec<u8> = sqlx::query_scalar(
            "SELECT cml_head FROM fleet_attempts WHERE community_id=$1 AND task_id=$2",
        )
        .bind(f.community.as_uuid())
        .bind(task.id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(head, start.id.as_bytes().as_slice());
    }
    let valid = f.event(&task, CmlTransition::Submit, Some(&start));
    assert!(f.persist(&valid).await.unwrap().1);
    assert!(!f.persist(&valid).await.unwrap().1);
    assert_eq!(f.event_count(&valid).await, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn atomic_signed_plan_start_receipt_and_duplicate_round_trip() {
    let f = Fixture::new().await;
    let (plan, claim, mut task) = f.plan_claim().await;
    task.status = CmlStatus::Working;
    let start = f.event(&task, CmlTransition::Start, Some(&claim));
    let (one, two) = tokio::join!(f.persist(&start), f.persist(&start));
    assert_eq!(
        [one.unwrap().1, two.unwrap().1]
            .iter()
            .filter(|&&v| v)
            .count(),
        1
    );
    assert_eq!(f.event_count(&start).await, 1);
    assert_eq!(
        f.admission(&plan, &start).await.unwrap()["start_event_id"],
        start.id.to_hex()
    );
    let receipt = f.receipt(&plan, Some(&start), ReceiptStatus::Success);
    assert!(f.persist(&receipt).await.unwrap().1);
    assert!(!f.persist(&receipt).await.unwrap().1);
    let display =
        f.db.list_task_attempts(f.community, f.task.id)
            .await
            .unwrap();
    assert_eq!(display.len(), 1);
    assert_eq!(display[0]["state"], "success");
    assert_eq!(display[0]["receipt_event_id"], receipt.id.to_hex());
    assert_eq!(display[0]["receipt"]["id"], receipt.id.to_hex());
    assert!(f.admission(&plan, &start).await.is_err());
    f.db.validate_deletion_catalog().await.unwrap();
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn ungranted_plan_rolls_back_signed_event_and_projection() {
    let f = Fixture::new().await;
    let plan = f.event(&f.task, CmlTransition::Plan, None);
    assert!(f.persist(&plan).await.is_err());
    assert_eq!(f.event_count(&plan).await, 0);
    assert!(f
        .db
        .list_task_attempts(f.community, f.task.id)
        .await
        .unwrap()
        .is_empty());
    f.grant().await;
    assert!(f.persist(&plan).await.unwrap().1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn revoked_grant_serializes_before_start_and_rolls_back_event() {
    let f = Fixture::new().await;
    let (_, claim, mut task) = f.plan_claim().await;
    task.status = CmlStatus::Working;
    let start = f.event(&task, CmlTransition::Start, Some(&claim));
    let mut revocation = f.pool.begin().await.unwrap();
    sqlx::query("UPDATE agent_capability_grants SET revoked_at=clock_timestamp(),revoked_by=agent_pubkey WHERE community_id=$1")
        .bind(f.community.as_uuid()).execute(&mut *revocation).await.unwrap();
    let pending = f.persist(&start);
    tokio::pin!(pending);
    // The actual start is blocked on the positive grant row; the committed
    // revoke must then be re-evaluated, not trusted from an earlier snapshot.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut pending)
            .await
            .is_err()
    );
    revocation.commit().await.unwrap();
    assert!(pending.await.is_err());
    assert_eq!(f.event_count(&start).await, 0);
    assert_eq!(
        f.db.list_task_attempts(f.community, f.task.id)
            .await
            .unwrap()[0]["state"],
        "claimed"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn primary_admission_rechecks_grants_revision_home_membership_and_worker() {
    let f = Fixture::new().await;
    let (plan, start, _) = f.start().await;
    assert!(f.admission(&plan, &start).await.is_ok());
    let id = fleet::attempt_id(f.community, f.task.id, plan.id.as_bytes());
    assert!(f
        .db
        .fleet_start_admission(
            f.community,
            f.task.id,
            &id,
            start.id.as_bytes(),
            &f.planner.public_key().to_bytes()
        )
        .await
        .is_err());
    for statement in [
        "UPDATE agent_capability_grants SET revoked_at=clock_timestamp(),revoked_by=agent_pubkey WHERE community_id=$1",
        "UPDATE users SET machine_id='rosie' WHERE community_id=$1 AND agent_type IS NOT NULL",
        "UPDATE channel_members SET removed_at=clock_timestamp() WHERE community_id=$1 AND role='bot'",
        "UPDATE tasks SET title='changed approved task' WHERE community_id=$1",
        "UPDATE channels SET archived_at=clock_timestamp() WHERE community_id=$1",
        "UPDATE users SET deactivated_at=clock_timestamp() WHERE community_id=$1 AND agent_type IS NOT NULL",
    ] {
        let other=Fixture::new().await;let (p,s,_)=other.start().await;
        sqlx::query(sqlx::AssertSqlSafe(statement)).bind(other.community.as_uuid()).execute(&other.pool).await.unwrap();
        assert!(other.admission(&p,&s).await.is_err(),"guard failed for {statement}");
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn cancellation_is_only_intent_until_worker_terminal_receipt() {
    let f = Fixture::new().await;
    let (plan, start, mut task) = f.start().await;
    task.status = CmlStatus::Cancelled;
    task.lease = None;
    let cancel = f.event(&task, CmlTransition::Cancel, Some(&start));
    f.persist(&cancel).await.unwrap();
    let display =
        f.db.list_task_attempts(f.community, f.task.id)
            .await
            .unwrap();
    assert_eq!(display[0]["state"], "started");
    assert!(display[0]["receipt_event_id"].is_null());
    assert!(f.admission(&plan, &start).await.is_err());
    let unknown = f.receipt(&plan, Some(&start), ReceiptStatus::Unknown);
    f.persist(&unknown).await.unwrap();
    assert_eq!(
        f.db.list_task_attempts(f.community, f.task.id)
            .await
            .unwrap()[0]["state"],
        "unknown"
    );
    let receipt = f.receipt(&plan, Some(&start), ReceiptStatus::Cancelled);
    f.persist(&receipt).await.unwrap();
    assert_eq!(
        f.db.list_task_attempts(f.community, f.task.id)
            .await
            .unwrap()[0]["state"],
        "cancelled"
    );
    let mut retry = task.clone();
    retry.status = CmlStatus::Claimed;
    retry.lease = Some(buzz_core::cml::Lease {
        id: fleet::attempt_id(f.community, f.task.id, plan.id.as_bytes()),
        holder: f.worker.public_key().to_hex(),
        issued_at: retry.updated_at,
        expires_at: retry.updated_at + 300,
    });
    let retry = f.event(&retry, CmlTransition::Claim, Some(&cancel));
    assert!(f.persist(&retry).await.is_err());
    assert_eq!(f.event_count(&retry).await, 0);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn wrong_tenant_and_worker_receipts_do_not_modify_attempt() {
    let f = Fixture::new().await;
    let (plan, start, _) = f.start().await;
    let wrong = Fixture::new().await;
    let receipt = f.receipt(&plan, Some(&start), ReceiptStatus::Success);
    assert!(wrong
        .db
        .insert_fleet_event(wrong.community, &receipt, Some(f.channel), None)
        .await
        .is_err());
    let forged = nostr::EventBuilder::new(receipt.kind, receipt.content.clone())
        .tags(receipt.tags.clone())
        .custom_created_at(receipt.created_at)
        .sign_with_keys(&f.planner)
        .unwrap();
    assert!(f.persist(&forged).await.is_err());
    assert_eq!(f.event_count(&forged).await, 0);
    assert_eq!(
        f.db.list_task_attempts(f.community, f.task.id)
            .await
            .unwrap()[0]["state"],
        "started"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn migration_schema_upgrade_keeps_existing_events_but_never_backfills_execution_authority() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    crate::migration::run_migrations_through(&pool, 50)
        .await
        .unwrap();
    let f = Fixture::new().await;
    f.grant().await;
    let plan = f.event(&f.task, CmlTransition::Plan, None);
    f.db.insert_event_with_thread_metadata(f.community, &plan, Some(f.channel), None)
        .await
        .unwrap();
    crate::migration::run_migrations(&pool).await.unwrap();
    assert_eq!(f.event_count(&plan).await, 1);
    assert!(!f.persist(&plan).await.unwrap().1);
    assert!(f
        .db
        .list_task_attempts(f.community, f.task.id)
        .await
        .unwrap()
        .is_empty());
    f.db.validate_deletion_catalog().await.unwrap();
}
