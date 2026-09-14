use super::*;
async fn setup_pool() -> PgPool {
    PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("connect to test DB")
}

async fn make_test_community(pool: &PgPool) -> CommunityId {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(id)
        .bind(format!("task-test-{}.example", id.simple()))
        .execute(pool)
        .await
        .expect("insert test community");
    CommunityId::from_uuid(id)
}

async fn make_test_user(pool: &PgPool, community: CommunityId, seed: u8) -> Vec<u8> {
    let pubkey = vec![seed; 32];
    crate::user::ensure_user(pool, community, &pubkey)
        .await
        .expect("ensure test user");
    pubkey
}

async fn delete_test_community(pool: &PgPool, community: CommunityId) {
    for table in ["task_events", "tasks", "users"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE community_id = $1"
        )))
        .bind(community.as_uuid())
        .execute(pool)
        .await
        .expect("delete test rows");
    }
    sqlx::query("DELETE FROM communities WHERE id = $1")
        .bind(community.as_uuid())
        .execute(pool)
        .await
        .expect("delete test community");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn create_then_list_then_get_round_trips_a_task_and_its_history() {
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let creator = make_test_user(&pool, community, 0x11).await;

    let created = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(creator.clone()),
            title: "ship the task system".to_owned(),
            body: Some("phase 1".to_owned()),
            priority: 5,
            source: Some("claude".to_owned()),
            ..NewTask::default()
        },
    )
    .await
    .expect("create task");

    assert_eq!(created.title, "ship the task system");
    assert_eq!(created.status, TaskStatus::Todo);
    assert_eq!(created.priority, 5);
    assert_eq!(created.done_at, None);
    assert_eq!(
        created.created_by_pubkey.as_deref(),
        Some(creator.as_slice())
    );

    let listed = list_tasks(
        &pool,
        community,
        &TaskFilter {
            limit: 10,
            ..TaskFilter::default()
        },
    )
    .await
    .expect("list tasks");
    assert_eq!(listed, vec![created.clone()]);

    let fetched = get_task(&pool, community, created.id)
        .await
        .expect("get task");
    assert_eq!(fetched, created);

    // create_task commits the task and its opening history entry together.
    let events = list_task_events(&pool, community, created.id)
        .await
        .expect("list events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, TaskAction::Created);

    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_task_is_findable_by_its_source_ref() {
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let creator = make_test_user(&pool, community, 0x21).await;

    let wanted = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(creator.clone()),
            title: "from the thread we care about".to_owned(),
            source: Some("app".to_owned()),
            source_ref: Some("thread-head-aaa".to_owned()),
            ..NewTask::default()
        },
    )
    .await
    .expect("create linked task");

    let other = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(creator.clone()),
            title: "from a different thread".to_owned(),
            source: Some("app".to_owned()),
            source_ref: Some("thread-head-bbb".to_owned()),
            ..NewTask::default()
        },
    )
    .await
    .expect("create unrelated task");

    // Exact equality: the reader queries the same key the writer wrote.
    let found = list_tasks(
        &pool,
        community,
        &TaskFilter {
            source_ref: Some("thread-head-aaa".to_owned()),
            limit: 10,
            ..TaskFilter::default()
        },
    )
    .await
    .expect("list by source_ref");
    assert_eq!(found, vec![wanted.clone()]);

    // An unknown reference is an empty page, never an error and never a
    // fallback to "everything".
    let missing = list_tasks(
        &pool,
        community,
        &TaskFilter {
            source_ref: Some("thread-head-does-not-exist".to_owned()),
            limit: 10,
            ..TaskFilter::default()
        },
    )
    .await
    .expect("list unknown source_ref");
    assert!(missing.is_empty());

    // Omitting the filter must keep today's behaviour: both tasks.
    let unfiltered = list_tasks(
        &pool,
        community,
        &TaskFilter {
            limit: 10,
            ..TaskFilter::default()
        },
    )
    .await
    .expect("list unfiltered");
    assert_eq!(unfiltered.len(), 2);
    assert!(unfiltered.contains(&wanted));
    assert!(unfiltered.contains(&other));

    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_status_change_sets_done_at_and_appends_exactly_one_event() {
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let creator = make_test_user(&pool, community, 0x22).await;

    let task = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(creator.clone()),
            title: "finish it".to_owned(),
            ..NewTask::default()
        },
    )
    .await
    .expect("create task");

    let done = update_task(
        &pool,
        community,
        task.id,
        &TaskPatch {
            status: Some(TaskStatus::Done),
            ..TaskPatch::default()
        },
        Some(&creator),
    )
    .await
    .expect("mark done");
    assert_eq!(done.status, TaskStatus::Done);
    assert!(
        done.done_at.is_some(),
        "done_at is derived from the status, not supplied by the caller"
    );

    let events = list_task_events(&pool, community, task.id)
        .await
        .expect("list events");
    assert_eq!(events.len(), 2, "created + status_changed");
    assert_eq!(events[1].action, TaskAction::StatusChanged);
    assert_eq!(events[1].from_status, Some(TaskStatus::Todo));
    assert_eq!(events[1].to_status, Some(TaskStatus::Done));

    // Restating the same status is idempotent: no second event.
    update_task(
        &pool,
        community,
        task.id,
        &TaskPatch {
            status: Some(TaskStatus::Done),
            ..TaskPatch::default()
        },
        Some(&creator),
    )
    .await
    .expect("restate done");
    let events = list_task_events(&pool, community, task.id)
        .await
        .expect("list events again");
    assert_eq!(events.len(), 2, "restating a status must append nothing");

    // Reopening clears done_at, keeping chk_tasks_done_at_matches_status
    // satisfiable.
    let reopened = update_task(
        &pool,
        community,
        task.id,
        &TaskPatch {
            status: Some(TaskStatus::Todo),
            ..TaskPatch::default()
        },
        Some(&creator),
    )
    .await
    .expect("reopen");
    assert_eq!(reopened.done_at, None);

    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_task_id_is_invisible_to_another_community() {
    let pool = setup_pool().await;
    let owner = make_test_community(&pool).await;
    let stranger = make_test_community(&pool).await;
    let creator = make_test_user(&pool, owner, 0x33).await;

    let task = create_task(
        &pool,
        owner,
        NewTask {
            created_by_pubkey: Some(creator),
            title: "tenant-private".to_owned(),
            ..NewTask::default()
        },
    )
    .await
    .expect("create task");

    // The bare id is not a capability: presented against another tenant it
    // reads as absent, never as the owner's row.
    assert!(matches!(
        get_task(&pool, stranger, task.id).await,
        Err(DbError::NotFound(_))
    ));
    assert!(matches!(
        append_task_event(
            &pool,
            stranger,
            task.id,
            None,
            TaskAction::Commented,
            Some("leak?")
        )
        .await,
        Err(DbError::NotFound(_))
    ));

    delete_test_community(&pool, owner).await;
    delete_test_community(&pool, stranger).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_task_keeps_at_most_one_persisted_summary() {
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let actor = make_test_user(&pool, community, 0x44).await;

    let task = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(actor.clone()),
            title: "summarize me".to_owned(),
            ..NewTask::default()
        },
    )
    .await
    .expect("create task");

    append_task_event(
        &pool,
        community,
        task.id,
        Some(&actor),
        TaskAction::SummaryPersisted,
        Some("first summary"),
    )
    .await
    .expect("first summary");

    let second = append_task_event(
        &pool,
        community,
        task.id,
        Some(&actor),
        TaskAction::SummaryPersisted,
        Some("second summary"),
    )
    .await;
    assert!(
        matches!(second, Err(DbError::InvalidData(_))),
        "the partial unique index must reject a second summary, got {second:?}"
    );

    // Ordinary comments stay unbounded.
    for _ in 0..2 {
        append_task_event(
            &pool,
            community,
            task.id,
            Some(&actor),
            TaskAction::Commented,
            Some("a comment"),
        )
        .await
        .expect("comment");
    }

    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn assignment_and_schedule_history_preserves_before_after_and_noop_retry() {
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let actor = make_test_user(&pool, community, 0x51).await;
    let assignee = make_test_user(&pool, community, 0x52).await;
    let due: DateTime<Utc> = "2026-09-09T14:15:16.123456Z".parse().expect("date");
    let task = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(actor.clone()),
            title: "original".into(),
            ..NewTask::default()
        },
    )
    .await
    .expect("create");
    let patch = TaskPatch {
        title: Some("renamed".into()),
        assignee_pubkey: Some(Some(assignee.clone())),
        priority: Some(8),
        due_at: Some(Some(due + chrono::TimeDelta::nanoseconds(789))),
        ..TaskPatch::default()
    };
    let updated = update_task(&pool, community, task.id, &patch, Some(&actor))
        .await
        .expect("update");
    assert_eq!(updated.assignee_pubkey, Some(assignee.clone()));
    assert_eq!(updated.priority, 8);
    assert_eq!(updated.due_at, Some(due));
    let events = list_task_events(&pool, community, task.id)
        .await
        .expect("history");
    assert_eq!(events.len(), 5);
    assert_eq!(
        events[0].changes.as_ref().expect("initial snapshot")["priority"],
        json!({"from": null, "to": 0})
    );
    for (action, changes) in [
        (
            TaskAction::TitleChanged,
            json!({"title": {"from": "original", "to": "renamed"}}),
        ),
        (
            TaskAction::Assigned,
            json!({"assignee": {"from": null, "to": hex::encode(&assignee)}}),
        ),
        (
            TaskAction::PriorityChanged,
            json!({"priority": {"from": 0, "to": 8}}),
        ),
        (
            TaskAction::DueAtChanged,
            json!({"due_at": {"from": null, "to": due}}),
        ),
    ] {
        let event = events
            .iter()
            .find(|e| e.action == action)
            .expect("field history");
        assert_eq!(event.changes, Some(changes));
        assert_eq!(event.actor_pubkey, Some(actor.clone()));
    }
    // A transport retry must change neither history nor pagination order.
    let retried = update_task(&pool, community, task.id, &patch, Some(&actor))
        .await
        .expect("retry");
    assert_eq!(retried, updated);
    assert_eq!(
        list_task_events(&pool, community, task.id)
            .await
            .expect("retry history"),
        events
    );

    update_task(
        &pool,
        community,
        task.id,
        &TaskPatch {
            assignee_pubkey: Some(Some(actor.clone())),
            ..TaskPatch::default()
        },
        Some(&actor),
    )
    .await
    .expect("reassign");
    update_task(
        &pool,
        community,
        task.id,
        &TaskPatch {
            assignee_pubkey: Some(None),
            due_at: Some(None),
            ..TaskPatch::default()
        },
        Some(&actor),
    )
    .await
    .expect("clear assignment and deadline");
    let events = list_task_events(&pool, community, task.id)
        .await
        .expect("cleared history");
    assert_eq!(
        events[5].changes,
        Some(json!({"assignee": {"from": hex::encode(&assignee), "to": hex::encode(&actor)}}))
    );
    assert_eq!(
        events[6].changes,
        Some(json!({"assignee": {"from": hex::encode(&actor), "to": null}}))
    );
    assert_eq!(
        events[7].changes,
        Some(json!({"due_at": {"from": due, "to": null}}))
    );
    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn task_mutation_rolls_back_when_its_history_cannot_be_written() {
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let actor = make_test_user(&pool, community, 0x53).await;
    let task = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(actor.clone()),
            title: "atomic mutation".into(),
            ..NewTask::default()
        },
    )
    .await
    .expect("create");
    // Deliberately fail the history insert after the task UPDATE. A user FK
    // violation is an existing production constraint; no test-only write seam.
    let result = update_task(
        &pool,
        community,
        task.id,
        &TaskPatch {
            priority: Some(99),
            ..TaskPatch::default()
        },
        Some(&[0xfe; 32]),
    )
    .await;
    assert!(
        result.is_err(),
        "invalid audit actor must reject the mutation"
    );
    assert_eq!(
        get_task(&pool, community, task.id)
            .await
            .expect("persisted task"),
        task
    );
    assert_eq!(
        list_task_events(&pool, community, task.id)
            .await
            .expect("history")
            .len(),
        1
    );
    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn concurrent_schedule_updates_record_the_locked_before_image() {
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let actor = make_test_user(&pool, community, 0x54).await;
    let task = create_task(
        &pool,
        community,
        NewTask {
            created_by_pubkey: Some(actor.clone()),
            title: "concurrent priority".into(),
            ..NewTask::default()
        },
    )
    .await
    .expect("create");
    let first = TaskPatch {
        priority: Some(5),
        ..TaskPatch::default()
    };
    let second = TaskPatch {
        priority: Some(7),
        ..TaskPatch::default()
    };
    let (a, b) = tokio::join!(
        update_task(&pool, community, task.id, &first, Some(&actor)),
        update_task(&pool, community, task.id, &second, Some(&actor)),
    );
    a.expect("first mutation");
    b.expect("second mutation");
    let events = list_task_events(&pool, community, task.id)
        .await
        .expect("history");
    // The normal history read must preserve the actual mutation order.
    let changes: Vec<_> = events
        .iter()
        .filter(|e| e.action == TaskAction::PriorityChanged)
        .map(|e| &e.changes.as_ref().expect("structured history")["priority"])
        .collect();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["from"], 0);
    assert_eq!(changes[1]["from"], changes[0]["to"]);
    assert_eq!(
        changes[1]["to"],
        get_task(&pool, community, task.id)
            .await
            .expect("task")
            .priority
    );
    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn visibility_precedes_limit_and_cursor_keeps_equal_timestamp_rows() {
    use buzz_core::channel::{ChannelType, ChannelVisibility};
    let pool = setup_pool().await;
    let community = make_test_community(&pool).await;
    let actor = make_test_user(&pool, community, 0x55).await;
    let channel = crate::channel::create_channel(
        &pool,
        community,
        "hidden",
        ChannelType::Stream,
        ChannelVisibility::Private,
        None,
        &actor,
        None,
    )
    .await
    .expect("channel");
    let mut visible = Vec::new();
    for title in ["visible one", "visible two", "visible three"] {
        visible.push(
            create_task(
                &pool,
                community,
                NewTask {
                    title: title.into(),
                    ..NewTask::default()
                },
            )
            .await
            .expect("visible task"),
        );
    }
    let same_time: DateTime<Utc> = "2026-09-07T10:11:12.123456Z".parse().expect("timestamp");
    sqlx::query("UPDATE tasks SET updated_at = $2 WHERE community_id = $1")
        .bind(community.as_uuid())
        .bind(same_time)
        .execute(&pool)
        .await
        .expect("equal timestamps");
    for _ in 0..3 {
        create_task(
            &pool,
            community,
            NewTask {
                title: "hidden newer".into(),
                channel_id: Some(channel.id),
                ..NewTask::default()
            },
        )
        .await
        .expect("hidden task");
    }
    visible.sort_by_key(|task| std::cmp::Reverse(task.id));
    let mut filter = TaskFilter {
        visible_channel_ids: Some(vec![]),
        limit: 2,
        ..TaskFilter::default()
    };
    let first = list_tasks(&pool, community, &filter)
        .await
        .expect("first visible page");
    assert_eq!(
        first.iter().map(|t| t.id).collect::<Vec<_>>(),
        visible[..2].iter().map(|t| t.id).collect::<Vec<_>>()
    );
    let last = first.last().expect("first page tail");
    filter.before = Some(TaskCursor {
        updated_at: last.updated_at,
        id: last.id,
    });
    let second = list_tasks(&pool, community, &filter)
        .await
        .expect("second page");
    assert_eq!(
        second.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![visible[2].id]
    );
    filter.before = Some(TaskCursor {
        updated_at: second[0].updated_at,
        id: second[0].id,
    });
    assert!(list_tasks(&pool, community, &filter)
        .await
        .expect("end of pages")
        .is_empty());
    filter.before = None;
    filter.visible_channel_ids = Some(vec![channel.id]);
    assert!(list_tasks(&pool, community, &filter)
        .await
        .expect("member page")
        .iter()
        .all(|t| t.channel_id == Some(channel.id)));
    // The per-test database is discarded by the runner, but normal cleanup
    // also leaves this scenario reusable in the focused local invocation.
    sqlx::query("DELETE FROM task_events WHERE community_id = $1")
        .bind(community.as_uuid())
        .execute(&pool)
        .await
        .expect("events cleanup");
    sqlx::query("DELETE FROM tasks WHERE community_id = $1")
        .bind(community.as_uuid())
        .execute(&pool)
        .await
        .expect("tasks cleanup");
    sqlx::query("DELETE FROM channel_members WHERE community_id = $1")
        .bind(community.as_uuid())
        .execute(&pool)
        .await
        .expect("members cleanup");
    sqlx::query("DELETE FROM channels WHERE community_id = $1")
        .bind(community.as_uuid())
        .execute(&pool)
        .await
        .expect("channels cleanup");
    delete_test_community(&pool, community).await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn migration_schema_task_history_upgrade_preserves_legacy_rows() {
    let pool = setup_pool().await;
    crate::migration::run_migrations_through(&pool, 46)
        .await
        .expect("legacy migrations");
    let community = make_test_community(&pool).await;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO tasks (community_id, title) VALUES ($1, 'legacy task') RETURNING id",
    )
    .bind(community.as_uuid())
    .fetch_one(&pool)
    .await
    .expect("legacy task");
    sqlx::query(
        "INSERT INTO task_events (community_id, task_id, action) VALUES ($1, $2, 'assigned')",
    )
    .bind(community.as_uuid())
    .bind(id)
    .execute(&pool)
    .await
    .expect("legacy history");
    crate::migration::run_migrations(&pool)
        .await
        .expect("upgrade migration");
    let legacy = list_task_events(&pool, community, id)
        .await
        .expect("read old history");
    assert_eq!(legacy.len(), 1);
    assert_eq!(
        legacy[0].changes, None,
        "must not fabricate past before-images"
    );
    update_task(
        &pool,
        community,
        id,
        &TaskPatch {
            priority: Some(9),
            ..TaskPatch::default()
        },
        None,
    )
    .await
    .expect("new write after upgrade");
    let history = list_task_events(&pool, community, id)
        .await
        .expect("read upgraded history");
    assert_eq!(
        history[1].changes,
        Some(json!({"priority": {"from": 0, "to": 9}}))
    );
    delete_test_community(&pool, community).await;
}
