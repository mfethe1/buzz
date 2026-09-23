#[test]
fn database_metrics_and_slow_logs_exclude_sensitive_or_unbounded_fields() {
    let implementation = include_str!("../src/runtime/observability.rs");
    let datastore_macro = include_str!("../../buzz-datastore-tracing/src/lib.rs");
    let instrumentation = format!("{implementation}\n{datastore_macro}");

    for forbidden in [
        "\"community\" =>",
        "\"event_id\" =>",
        "\"event_kind\" =>",
        "\"kind\" =>",
        "\"sql\" =>",
        "\"query\" =>",
        "\"query_id\" =>",
        "\"d_tag\" =>",
        "\"coordinate\" =>",
        "community =",
        "event_id =",
        "event_kind =",
        "sql =",
        "query_id =",
        "d_tag =",
        "coordinate =",
    ] {
        assert!(
            !instrumentation.contains(forbidden),
            "database instrumentation must not expose {forbidden}"
        );
    }

    assert!(datastore_macro.contains("name: LitStr"));
    assert!(datastore_macro.contains("\"operation\" => #name"));
    assert!(datastore_macro.contains("elapsed_ms ="));
    assert!(
        datastore_macro.contains("parent: None"),
        "slow warnings must not inherit dynamic datastore span fields"
    );
    // The runtime tracing-layer assertion covers field names because a source
    // search would also match ordinary local variables such as `record_error`.
}

#[test]
fn relay_admin_db_wrappers_have_exactly_one_datastore_span() {
    for (domain, source) in [
        (
            "relay_admin_actions",
            include_str!("../src/store/relay_admin_actions.rs"),
        ),
        (
            "relay_operators",
            include_str!("../src/store/relay_operators.rs"),
        ),
    ] {
        let db_impl = source
            .split_once("impl crate::Db {")
            .unwrap_or_else(|| panic!("{domain} must own its Db wrappers"))
            .1
            .split_once("\n#[cfg(test)]")
            .unwrap_or_else(|| panic!("{domain} Db wrappers must precede focused tests"))
            .0;
        let mut pending_spans = 0;
        let mut methods = 0;

        for line in db_impl.lines() {
            if line.contains("#[datastore_span(") {
                pending_spans += 1;
            }
            if line.trim_start().starts_with("pub async fn ") {
                assert_eq!(pending_spans, 1, "{domain} wrapper `{line}` span count");
                pending_spans = 0;
                methods += 1;
            }
        }

        assert!(methods > 0, "{domain} must own public Db wrappers");
        assert_eq!(
            pending_spans, 0,
            "{domain} has an unattached datastore span"
        );
    }
}

#[test]
fn p0_pool_acquisitions_use_typed_operation_pairs_without_other() {
    let observability = include_str!("../src/runtime/observability.rs");
    assert!(observability.contains("enum PoolOperation"));
    assert!(observability.contains("pub(crate) enum WriterOperation"));
    assert!(observability.contains("pub(crate) enum ReaderOperation"));
    assert!(observability.contains("Self::WriterAuthentication"));
    assert!(observability.contains("Self::ReaderSubscriptionHistory"));
    assert!(observability.contains("pub(crate) async fn acquire_writer("));
    assert!(observability.contains("pub(super) async fn acquire_reader_with_legacy_metrics("));
    assert!(observability.contains("static POOL_WAITERS: [Mutex<u64>"));
    assert!(!observability.contains("AtomicU64"));
    assert!(!observability.contains("DbOperation::Other"));
    assert!(!observability.contains("\"other\""));
    assert!(!observability.contains("buzz_db_pool_acquire_timeouts_total"));
    assert!(!observability.contains("\"result\" =>"));
    let legacy_transaction = observability
        .split_once("pub(crate) async fn begin_transaction(")
        .expect("observability must expose attributed transaction acquisition")
        .1
        .split_once("pub(crate) async fn observe_advisory_lock")
        .expect("transaction acquisition must precede advisory-lock observation")
        .0;
    assert!(legacy_transaction.contains("acquire_writer_with_legacy_metrics("));

    let runtime = include_str!("../src/runtime/mod.rs");
    assert!(runtime.contains("observability::acquire_writer_until("));
    assert!(runtime.contains("WriterOperation::Readiness"));
    assert!(runtime.contains("WriterOperation::EventWrite"));
    assert!(runtime.contains("ReaderOperation::Bootstrap"));
    assert!(runtime.contains("pub async fn begin_event_write_transaction"));
    let reader_boot = runtime
        .split_once("async fn read_pool_boot_ping_once(")
        .expect("runtime must expose the reader boot probe")
        .1
        .split_once("#[cfg(test)]")
        .expect("reader boot probe must precede its test seam")
        .0;
    assert!(reader_boot.contains("acquire_reader_with_legacy_metrics("));
    let routed_reader = runtime
        .split_once("async fn proved_reader(")
        .expect("runtime must expose the routed-reader checkout")
        .1
        .split_once("async fn reader_aurora_capability_on(")
        .expect("routed-reader checkout must precede capability probing")
        .0;
    assert!(routed_reader.contains("acquire_reader_with_legacy_metrics(read_pool, operation)"));
    let event_write_transaction = runtime
        .split_once("pub async fn begin_event_write_transaction(")
        .expect("runtime must expose the legacy event-write transaction seam")
        .1
        .split_once("pub async fn insert_event_with_serving_write_guard(")
        .expect("legacy event-write transaction must precede guarded writes")
        .0;
    assert!(event_write_transaction.contains("acquire_writer_with_legacy_metrics("));

    let migration = include_str!("../src/runtime/migration.rs");
    let migration_lock = migration
        .split_once("pub(crate) async fn with_exclusive_schema_destruction_lock")
        .expect("migration must expose the schema-safety acquisition seam")
        .1
        .split_once("async fn reject_legacy_nip_rs_cardinality_ambiguity")
        .expect("schema-safety acquisition must precede migration validation")
        .0;
    assert!(migration_lock.contains("acquire_writer_with_legacy_metrics("));

    let allowlist = include_str!("../src/store/allowlist.rs");
    assert!(allowlist.contains("WriterOperation::Authentication"));
    assert!(allowlist.contains("WriterOperation::Authorization"));
    assert!(!allowlist.contains("fetch_one(&self.pool)"));

    let event = include_str!("../src/store/event.rs");
    assert!(event.contains("query_events_with_operation"));
    assert!(event.contains("WriterOperation::Authorization"));
    assert!(event.contains("WriterOperation::SubscriptionHistory"));
    assert!(event.contains("ReaderOperation::SubscriptionHistory"));
    let backfill_d_tags = event
        .split_once("pub async fn backfill_d_tags")
        .expect("event store must expose the startup d-tag backfill")
        .1
        .split_once("/// Soft-delete NIP-29 discovery events")
        .expect("d-tag backfill must precede discovery deletion")
        .0;
    assert!(backfill_d_tags.contains("WriterOperation::Bootstrap"));
    assert!(backfill_d_tags.contains("execute(&mut *connection)"));
    let soft_delete_discovery = event
        .split_once("pub async fn soft_delete_discovery_events")
        .expect("event store must expose discovery-event deletion")
        .1
        .split_once("\n}\n\n#[cfg(test)]")
        .expect("discovery deletion must end the production Db implementation")
        .0;
    assert!(soft_delete_discovery.contains("WriterOperation::EventWrite"));
    assert!(soft_delete_discovery.contains("execute(&mut *connection)"));

    let side_effects = include_str!("../../buzz-relay/src/handlers/side_effects.rs");
    assert!(side_effects.contains("query_events_for_event_write"));
    assert!(side_effects.contains("query_events_for_bootstrap"));
    assert!(side_effects.contains(".list_channels_for_bootstrap("));

    let deletion = include_str!("../src/store/deletion.rs");
    let public_serving_catalog = deletion
        .split_once("pub async fn validate_serving_catalog(&self)")
        .expect("deletion store must preserve its public serving-catalog API")
        .1
        .split_once("async fn validate_serving_catalog_on")
        .expect("public serving-catalog validation must delegate to its connection helper")
        .0;
    assert!(public_serving_catalog.contains("WriterOperation::Bootstrap"));
    assert!(public_serving_catalog.contains("observability::acquire_writer("));
    assert!(public_serving_catalog.contains("validate_serving_catalog_on"));
    assert!(!public_serving_catalog.contains("self.pool.acquire()"));

    let thread = include_str!("../src/store/thread.rs");
    let thread_metadata = thread
        .split_once("pub async fn get_thread_metadata_by_event(")
        .expect("thread store must expose metadata lookup")
        .1
        .split_once("// -- Db API")
        .expect("metadata lookup must precede the Db wrapper section")
        .0;
    assert!(thread_metadata.contains("WriterOperation::EventWrite"));
    assert!(thread_metadata.contains("fetch_optional(&mut *connection)"));
    assert!(!thread_metadata.contains("fetch_optional(pool)"));

    let channel = include_str!("../src/store/channel.rs");
    assert!(channel.contains("async fn begin_event_write_transaction("));
    assert!(channel.contains("async fn acquire_event_write_connection("));
    for (start, end, expected) in [
        (
            "pub async fn create_channel(\n",
            "/// Creates a channel with a client-supplied UUID",
            "begin_event_write_transaction(pool)",
        ),
        (
            "pub async fn create_channel_with_id(\n",
            "/// Fetches a channel record by `(community_id, id)`",
            "begin_event_write_transaction(pool)",
        ),
        (
            "pub async fn update_channel(\n",
            "/// Sets the topic for a channel",
            "begin_event_write_transaction(pool)",
        ),
        (
            "pub async fn set_topic(\n",
            "/// Sets the purpose for a channel",
            "acquire_event_write_connection(pool)",
        ),
        (
            "pub async fn set_purpose(\n",
            "/// Archives a channel",
            "acquire_event_write_connection(pool)",
        ),
        (
            "pub async fn archive_channel(\n",
            "/// Unarchives a channel",
            "acquire_event_write_connection(pool)",
        ),
        (
            "pub async fn unarchive_channel(\n",
            "/// Soft-delete a channel",
            "acquire_event_write_connection(pool)",
        ),
        (
            "pub async fn soft_delete_channel(\n",
            "/// Archive ephemeral channels",
            "acquire_event_write_connection(pool)",
        ),
    ] {
        let function = channel
            .split_once(start)
            .unwrap_or_else(|| panic!("missing channel seam {start}"))
            .1
            .split_once(end)
            .unwrap_or_else(|| panic!("channel seam {start} must precede {end}"))
            .0;
        assert!(
            function.contains(expected),
            "channel seam {start} must use {expected}"
        );
        assert!(!function.contains("pool.begin().await"));
        assert!(!function.contains(".execute(pool)"));
        assert!(!function.contains(".fetch_optional(pool)"));
    }
    let get_channel = channel
        .split_once("async fn get_channel_with_operation(")
        .expect("channel store must route shared lookups through caller-owned intent")
        .1
        .split_once("/// Returns the canvas content")
        .expect("channel lookup helper must precede canvas reads")
        .0;
    assert!(get_channel.contains("acquire_writer(pool, operation)"));
    assert!(get_channel.contains("fetch_optional(&mut *connection)"));
    assert!(!get_channel.contains("fetch_optional(pool)"));
    assert!(channel.contains("pub async fn get_channel_for_event_write("));
    let list_channels = channel
        .split_once("async fn list_channels_with_operation(")
        .expect("channel listing must accept caller-owned intent")
        .1
        .split_once("/// A channel archived by the ephemeral-channel reaper")
        .expect("channel listing must precede ephemeral-channel types")
        .0;
    assert!(list_channels.contains("acquire_writer(pool, operation)"));
    assert!(list_channels.contains("fetch_all(&mut *connection)"));
    assert!(!list_channels.contains("fetch_all(pool)"));
    assert!(channel.contains("pub async fn list_channels_for_bootstrap("));

    let channel_members = include_str!("../src/store/channel_members.rs");
    assert!(channel_members.contains("async fn get_members_with_operation("));
    assert!(channel_members.contains("pub async fn get_members_for_event_write("));
    assert!(channel_members.contains("async fn get_users_bulk_with_operation("));
    assert!(channel_members.contains("pub async fn get_users_bulk_for_event_write("));

    let huddle_link = event
        .split_once("async fn huddle_started_link_exists_with_operation(")
        .expect("huddle link lookup must accept caller-owned intent")
        .1
        .split_once("/// Insert a Nostr event")
        .expect("huddle link lookup must precede event insertion")
        .0;
    assert!(huddle_link.contains("acquire_writer(pool, operation)"));
    assert!(event.contains("pub async fn huddle_started_link_exists_for_event_write("));
    let ingest = include_str!("../../buzz-relay/src/handlers/ingest.rs");
    assert!(ingest.contains(".huddle_started_link_exists_for_event_write("));
    let audio = include_str!("../../buzz-relay/src/audio/handler.rs");
    assert!(audio.contains(".huddle_started_link_exists("));

    let workflow_sink = include_str!("../../buzz-relay/src/workflow_sink.rs");
    assert!(workflow_sink.contains(".get_members_for_event_write("));
    assert!(workflow_sink.contains(".get_users_bulk_for_event_write("));

    for write_caller in [
        include_str!("../../buzz-relay/src/handlers/side_effects.rs"),
        include_str!("../../buzz-relay/src/handlers/ingest.rs"),
        include_str!("../../buzz-relay/src/handlers/command_executor.rs"),
        workflow_sink,
    ] {
        assert!(!write_caller.contains(".get_channel("));
        assert!(write_caller.contains(".get_channel_for_event_write("));
    }

    let user = include_str!("../src/store/user.rs");
    let agent_channel_policy = user
        .split_once("pub async fn get_agent_channel_policy(")
        .expect("user store must expose get_agent_channel_policy")
        .1
        .split_once("/// Check whether `actor_pubkey`")
        .expect("agent policy lookup must precede owner lookup")
        .0;
    assert!(agent_channel_policy.contains("WriterOperation::Authorization"));
    assert!(agent_channel_policy.contains("fetch_optional(&mut *connection)"));
    assert!(!agent_channel_policy.contains("fetch_optional(pool)"));
    let is_agent_owner = user
        .split_once("pub async fn is_agent_owner(")
        .expect("user store must expose is_agent_owner")
        .1
        .split_once("/// Set the channel_add_policy")
        .expect("is_agent_owner must precede set_agent_channel_policy")
        .0;
    assert!(is_agent_owner.contains("WriterOperation::Authorization"));
    assert!(is_agent_owner.contains("acquire_writer("));
    assert!(is_agent_owner.contains("fetch_optional(&mut *connection)"));
    assert!(!is_agent_owner.contains("fetch_optional(pool)"));

    let moderation = include_str!("../src/store/moderation.rs");
    let restriction_state = moderation
        .split_once("pub async fn restriction_state(")
        .expect("moderation store must expose restriction_state")
        .1
        .split_once("/// Fetch the full ban/timeout row")
        .expect("restriction state must precede full ban reads")
        .0;
    assert!(restriction_state.contains("WriterOperation::Authorization"));
    assert!(restriction_state.contains("fetch_optional(&mut *connection)"));
    assert!(!restriction_state.contains("fetch_optional(pool)"));

    let community_store = include_str!("../src/store/community.rs");
    let ensure_community = community_store
        .split_once("pub async fn ensure_configured_community(")
        .expect("community store must expose ensure_configured_community")
        .1
        .split_once("/// Atomically creates a community")
        .expect("configured-community helpers must precede community creation")
        .0;
    assert!(ensure_community.contains("WriterOperation::Authorization"));
    assert!(ensure_community.contains("WriterOperation::Bootstrap"));
    assert!(ensure_community.contains("ensure_configured_community_with_operation"));
    assert!(ensure_community.contains("acquire_writer(&self.pool, operation)"));
    assert!(ensure_community.contains("fetch_optional(&mut *connection)"));
    let management_lookup = community_store
        .split_once("pub async fn lookup_community_by_host_for_management(")
        .expect("community store must expose management host lookup")
        .1
        .split_once("/// Lists communities where")
        .expect("management lookup must precede owner listing")
        .0;
    assert!(management_lookup.contains("WriterOperation::Authorization"));
    assert!(management_lookup.contains("fetch_optional(&mut *connection)"));
    assert!(!management_lookup.contains("fetch_optional(&self.pool)"));
    let community_production = community_store
        .split("\n#[cfg(test)]")
        .next()
        .expect("community production source");
    for required in [
        "WriterOperation::TenantResolution",
        "WriterOperation::Authorization",
        "WriterOperation::SubscriptionHistory",
        "WriterOperation::EventWrite",
    ] {
        assert!(
            community_production.contains(required),
            "community P0 paths must include {required} attribution"
        );
    }
    assert!(!community_production.contains("self.pool.begin().await"));
    assert!(!community_production.contains(".fetch_one(&self.pool)"));
    assert!(!community_production.contains(".fetch_all(&self.pool)"));
    assert!(!community_production.contains(".execute(&self.pool)"));
    // Only the NIP-11 presentation scalars are exempt from attributed checkout:
    // they are unauthenticated public-metadata reads, outside the P0 set. Naming
    // them beats counting them — a count says how many raw checkouts are tolerated
    // but not which, so adding one to a real P0 path reads as the same failure as
    // adding a fourth colour field.
    const NIP11_METADATA_READS: [&str; 2] = ["get_community_icon", "get_community_brand_color"];
    for method in community_production.split("    pub async fn ").skip(1) {
        let name = method
            .split_once(['(', '<'])
            .expect("method header must name a function")
            .0;
        assert!(
            !method.contains("(&self.pool)") || NIP11_METADATA_READS.contains(&name),
            "{name} takes a raw pool checkout; P0 paths must acquire an attributed \
             connection, and only {NIP11_METADATA_READS:?} are exempt"
        );
    }

    let thread_summary = thread
        .split_once("pub async fn get_thread_summary(")
        .expect("thread store must expose get_thread_summary")
        .1
        .split_once("/// Fetch one channel window")
        .expect("thread summary must precede channel-window reads")
        .0;
    assert!(thread_summary.contains("WriterOperation::EventWrite"));
    assert!(thread_summary.contains("fetch_optional(&mut *connection)"));
    assert!(thread_summary.contains("fetch_all(&mut *connection)"));
    assert!(!thread_summary.contains("fetch_optional(pool)"));
    assert!(!thread_summary.contains("fetch_all(pool)"));

    let archived_identities = include_str!("../src/store/archived_identities.rs");
    let archived_identity_production = archived_identities
        .split("\n#[cfg(test)]")
        .next()
        .expect("archived identity production source");
    assert_eq!(
        archived_identity_production
            .matches("WriterOperation::EventWrite")
            .count(),
        4,
        "all four archived identity operations must be attributed to event writes"
    );
    assert!(!archived_identity_production.contains("fetch_optional(pool)"));
    assert!(!archived_identity_production.contains("fetch_all(pool)"));
    assert!(!archived_identity_production.contains("execute(pool)"));

    let relay_main = include_str!("../../buzz-relay/src/main.rs");
    assert!(relay_main.contains("pool_state.db.refresh_pool_waiter_metrics();"));
    assert!(relay_main.contains(".ensure_configured_community_for_bootstrap("));

    let runtime = include_str!("../src/runtime/mod.rs");
    assert!(runtime.contains("observability::refresh_pool_waiters(self.read_pool.is_some())"));
    assert!(runtime.contains("self.verify_replica_fence_at_boot().await?"));
    let fence_boot = runtime
        .split_once("pub(crate) async fn verify_replica_fence_at_boot")
        .expect("runtime must expose attributed boot fence verification")
        .1
        .split_once("/// The pool for lag-tolerant reads")
        .expect("boot fence verification must precede routed-read plumbing")
        .0;
    assert!(fence_boot.contains("WriterOperation::Bootstrap"));

    let replica_fence = include_str!("../src/runtime/replica_fence.rs");
    let replica_fence_production = replica_fence
        .split("\n#[cfg(test)]")
        .next()
        .expect("replica-fence production source");
    assert!(replica_fence_production.contains("WriterOperation::Bootstrap"));
    assert!(replica_fence_production.contains("WriterOperation::Maintenance"));
    assert!(!replica_fence_production.contains("pool.begin().await"));
    assert!(!replica_fence_production.contains("writer.acquire().await"));
    assert!(!replica_fence_production.contains("fetch_optional(writer)"));

    let usage = include_str!("../src/store/usage.rs");
    let usage_production = usage
        .split("\n#[cfg(test)]")
        .next()
        .expect("usage production source");
    let usage_leader_lock = usage_production
        .split_once("pub async fn try_lock_usage_metrics(")
        .expect("usage store must expose the legacy leader-lock acquisition")
        .1
        .split_once("pub async fn usage_community_count(")
        .expect("usage leader lock must precede counter reads")
        .0;
    assert!(usage_leader_lock.contains("acquire_writer_with_legacy_metrics("));
    assert!(
        usage_production
            .matches("WriterOperation::Maintenance")
            .count()
            >= 11,
        "every periodic usage checkout must be maintenance-attributed"
    );
    for bypass in [
        ".fetch_one(pool)",
        ".fetch_all(pool)",
        ".fetch_optional(pool)",
        ".execute(pool)",
    ] {
        assert!(
            !usage_production.contains(bypass),
            "usage production path bypasses operation attribution with {bypass}"
        );
    }

    let channel_reaper = channel
        .split_once("pub async fn reap_expired_ephemeral_channels(pool:")
        .expect("channel store must expose ephemeral reaper")
        .1
        .split_once("\nimpl Db {")
        .expect("ephemeral reaper must precede Db wrappers")
        .0;
    assert!(channel_reaper.contains("WriterOperation::Maintenance"));
    assert!(channel_reaper.contains("fetch_all(&mut *connection)"));

    let deletion = include_str!("../src/store/deletion.rs");
    let lease_reaper = deletion
        .split_once("pub async fn reap_expired_serving_write_leases")
        .expect("deletion store must expose serving-lease reaper")
        .1
        .split_once("/// Return serving-lease counts")
        .expect("serving-lease reaper must precede stats")
        .0;
    assert!(lease_reaper.contains("WriterOperation::Maintenance"));
    assert!(lease_reaper.contains("execute(&mut *connection)"));
    let lease_stats = deletion
        .split_once("pub async fn serving_lease_stats")
        .expect("deletion store must expose serving-lease stats")
        .1
        .split_once("/// Whether a community remains active")
        .expect("serving-lease stats must precede serving-state reads")
        .0;
    assert!(lease_stats.contains("WriterOperation::Maintenance"));
    assert!(lease_stats.contains("fetch_one(&mut *connection)"));
    for (start, end) in [
        (
            "pub async fn acquire_serving_write_lease",
            "/// Renew an already-admitted external side-effect lease",
        ),
        (
            "pub async fn renew_serving_write_lease",
            "/// Release a serving side-effect lease",
        ),
        (
            "pub async fn release_serving_write_lease",
            "/// Check that an external side-effect lease remains current",
        ),
        (
            "pub async fn verify_serving_write_lease",
            "/// Delete expired serving leases",
        ),
        (
            "pub async fn is_serving_active",
            "async fn advance_with_checkpoint",
        ),
    ] {
        let function = deletion
            .split_once(start)
            .unwrap_or_else(|| panic!("missing serving-write seam {start}"))
            .1
            .split_once(end)
            .unwrap_or_else(|| panic!("serving-write seam {start} must precede {end}"))
            .0;
        assert!(
            function.contains("WriterOperation::EventWrite"),
            "serving-write seam {start} must be event-write attributed"
        );
        assert!(!function.contains("self.pool.begin().await"));
        assert!(!function.contains(".execute(&self.pool)"));
        assert!(!function.contains(".fetch_one(&self.pool)"));
    }

    let ensure_authorization = user
        .split_once("pub async fn ensure_user_for_authorization(")
        .expect("user store must expose NIP-OA authorization ensure")
        .1
        .split_once("/// Get a single user record")
        .expect("authorization ensure must precede generic user reads")
        .0;
    assert!(ensure_authorization.contains("WriterOperation::Authorization"));
    let set_owner_authorization = user
        .split_once("pub async fn set_agent_owner_for_authorization(")
        .expect("user store must expose NIP-OA authorization owner write")
        .1
        .split_once("/// Get the channel_add_policy")
        .expect("authorization owner write must precede policy reads")
        .0;
    assert!(set_owner_authorization.contains("WriterOperation::Authorization"));
    let relay_api = include_str!("../../buzz-relay/src/api/mod.rs");
    assert!(relay_api.contains(".ensure_user_for_authorization("));
    assert!(relay_api.contains(".set_agent_owner_for_authorization("));

    for (domain, source) in [
        (
            "channel_members",
            include_str!("../src/store/channel_members.rs"),
        ),
        ("archived_identities", archived_identities),
        ("event", event),
        ("git_repo", include_str!("../src/store/git_repo.rs")),
        ("push", include_str!("../src/store/push.rs")),
        ("replica_fence", replica_fence),
        ("reaction", include_str!("../src/store/reaction.rs")),
        ("relay_invite", include_str!("../src/store/relay_invite.rs")),
        (
            "relay_members",
            include_str!("../src/store/relay_members.rs"),
        ),
        ("thread", thread),
        (
            "relay_operators",
            include_str!("../src/store/relay_operators.rs"),
        ),
        ("usage", usage),
    ] {
        let production = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        for bypass in [
            "pool.begin().await",
            "self.pool.begin().await",
            ".fetch_one(pool)",
            ".fetch_one(&self.pool)",
            ".fetch_all(pool)",
            ".fetch_all(&self.pool)",
            ".fetch_optional(pool)",
            ".fetch_optional(&self.pool)",
            ".execute(pool)",
            ".execute(&self.pool)",
        ] {
            assert!(
                !production.contains(bypass),
                "{domain} production path bypasses operation attribution with {bypass}"
            );
        }
    }
}

/// REG-8: the channel write policy is a security gate, so every SELECT whose
/// row becomes a `ChannelRecord` must project `write_policy`. A query that
/// omits it used to yield the permissive default, which silently re-opened
/// every restricted channel; the parsers now error instead, and this guard
/// catches the omission at build time rather than leaving it to a
/// live-database run.
///
/// Three hardening lessons are encoded here, each from a guard that passed
/// while broken:
///   * the anchor matches BOTH bare (`SELECT id, ...`) and table-aliased
///     (`SELECT c.id, ...`) projections — keying on the bare form alone missed
///     `get_accessible_channels`;
///   * queries are identified by their `FROM channels` site, because nested
///     SELECTs yield several overlapping windows over one query and a
///     surviving window can mask a real omission;
///   * select lists containing a nested `SELECT` are REJECTED outright rather
///     than scanned — a scalar subquery that mentions `write_policy` would
///     otherwise satisfy the check for an outer projection that omits it.
///
/// The file list is globbed from `src/store/`, not hardcoded: a channel-record
/// SELECT added in a new store module must not escape the guard by living in a
/// file nobody remembered to list.
#[test]
fn every_channel_record_select_projects_write_policy() {
    let store_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store");
    let mut sources: Vec<(String, String)> = std::fs::read_dir(&store_dir)
        .expect("src/store must be readable")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "rs" {
                return None;
            }
            let label = path.file_name()?.to_string_lossy().into_owned();
            Some((label, std::fs::read_to_string(&path).ok()?))
        })
        .collect();
    sources.sort();
    assert!(
        sources.len() >= 20,
        "expected the store module glob to find the store sources, found {}; \
         the guard is not scanning what it claims",
        sources.len()
    );

    let mut checked = 0usize;
    let mut sites: Vec<String> = Vec::new();
    for (label, source) in &sources {
        // One entry per `FROM channels` occurrence => one entry per query.
        for (from_at, _) in source.match_indices("FROM channels") {
            let head = &source[..from_at];
            let Some(sel_at) = head.rfind("SELECT ") else {
                continue;
            };
            let select_list = &source[sel_at..from_at];
            // A channel-record projection always carries both enum casts; this
            // filters out counts, id-only and unrelated projections.
            if !(select_list.contains("channel_type::text")
                && select_list.contains("visibility::text"))
            {
                continue;
            }
            let line = source[..sel_at].matches('\n').count() + 1;

            // A nested SELECT inside the select list means `rfind` may have
            // latched onto the subquery, leaving the OUTER projection
            // unexamined. Refuse to certify rather than guess.
            assert!(
                !select_list[7..].contains("SELECT "),
                "{label}:{line}: channel-record select list contains a nested \
                 SELECT; this guard cannot verify the outer projection. Hoist \
                 the subquery out of the select list."
            );

            checked += 1;
            sites.push(format!("{label}:{line}"));
            assert!(
                select_list.contains("write_policy::text AS write_policy"),
                "{label}:{line}: channel-record SELECT does not project \
                 write_policy; the row parser fails closed at runtime. Add \
                 `write_policy::text AS write_policy` to the select list."
            );
        }
    }

    // Guards the guard: if the anchor drifts, the loop above would vacuously
    // pass. 5 in channel.rs + 2 in channel_members.rs + 3 in dm.rs.
    assert_eq!(
        checked, 10,
        "expected 10 channel-record SELECTs, found {checked} at {sites:?}; \
         the anchor has drifted and this guard no longer checks what it claims"
    );

    // No `ChannelRecord` constructor may invent a policy instead of reading it.
    for (label, source) in &sources {
        assert!(
            !source.contains("let write_policy = crate::channel::ChannelWritePolicy::default()"),
            "{label}: a ChannelRecord parser hardcodes the permissive write \
             policy; parse the projected column instead"
        );
        assert!(
            !source.contains("Err(_) => ChannelWritePolicy::default()"),
            "{label}: a write-policy read error must not fall back to the \
             permissive default"
        );
    }
}

/// REG-8: the Postgres `channel_write_policy` enum and `ChannelWritePolicy`'s
/// `FromStr` must stay in lockstep. Both parsers now fail CLOSED on an
/// unrecognized value, which is correct for a security gate but means a new
/// enum value added by a migration alone would hard-error every read of a
/// channel using it — a fail-closed outage, the mirror image of the fail-open
/// this feature exists to fix. The schema is the source of truth, so this
/// derives the expected set from it rather than restating a literal list.
#[test]
fn write_policy_enum_matches_the_rust_parser() {
    let schema = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/schema.sql"),
    )
    .expect("schema.sql must be readable");

    let decl_at = schema
        .find("CREATE TYPE channel_write_policy AS ENUM")
        .expect("channel_write_policy enum must exist in the schema");
    let open = schema[decl_at..].find('(').expect("enum body") + decl_at;
    let close = schema[open..].find(')').expect("enum body end") + open;
    let mut sql_values: Vec<String> = schema[open + 1..close]
        .split(',')
        .map(|v| v.trim().trim_matches('\'').to_string())
        .filter(|v| !v.is_empty())
        .collect();
    sql_values.sort();
    assert!(
        !sql_values.is_empty(),
        "failed to parse the enum body; this guard is not checking anything"
    );

    for value in &sql_values {
        let parsed = value.parse::<buzz_core::channel::ChannelWritePolicy>();
        assert!(
            parsed.is_ok(),
            "schema.sql declares channel_write_policy value {value:?} but \
             ChannelWritePolicy::from_str rejects it; every read of a channel \
             using that policy would hard-error. Add it to the Rust enum."
        );
        assert_eq!(
            parsed.expect("checked ok").to_string(),
            *value,
            "round-trip mismatch for {value:?}: Display must emit the exact \
             stored value or writes will not round-trip"
        );
    }

    // And the reverse: no Rust variant may be unstorable.
    for variant in buzz_core::channel::ChannelWritePolicy::ALL {
        assert!(
            sql_values.contains(&variant.to_string()),
            "ChannelWritePolicy::{variant:?} has no matching value in the \
             schema's channel_write_policy enum; it can never be stored"
        );
    }
}
