//! Build-time inventory checks over the on-disk `migrations/` directory.
//!
//! Nothing in sqlx rejects two migration files that share a numeric version
//! prefix: `sqlx::migrate::MigrateError` has no duplicate-version variant and
//! the `migrate!` macro neither dedups nor errors. The collision is therefore
//! discovered at relay startup inside `run_migrations`, which holds the
//! exclusive `SCHEMA_DESTRUCTION_LOCK_KEY` for the whole run — the worst
//! possible place to learn that the version set is ambiguous.
//!
//! These checks move that discovery to `cargo test` on the pull request that
//! introduced the duplicate. They are test-only by construction: no production
//! startup path gains any code, no database connection is opened, and no
//! migration SQL is read, executed or interpreted — filenames only.
//!
//! Two independent checks, so neither can rot silently:
//!
//! 1. [`duplicate_versions`] scans the directory and fails if two `*.sql` files
//!    share a leading version, naming every colliding file.
//! 2. [`tests::sqlx_resolver_agrees_with_disk`] resolves the same directory
//!    through sqlx's own migration source and compares its version multiset to
//!    ours, so the guard cannot pass because *our* parser and *sqlx's* parser
//!    disagree about what a version is, and a future sqlx that silently dedups
//!    colliding versions is detected there as well as by check 1.
//!
//! Check 2 deliberately does NOT embed a second `Migrator`: the source lint
//! `migration_execution_cannot_bypass_schema_destruction_lock` in
//! `super::migration` permits exactly one embedding in the whole workspace, and
//! that lint is authoritative. See the handoff brief for what this costs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Absolute path of the checked-in `migrations/` directory.
fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../migrations")
        .canonicalize()
        .expect("migrations/ directory must exist relative to crates/buzz-db")
}

/// Leading decimal digits of a migration filename, e.g. `0049` in
/// `0049_task_event_changes.sql`. `None` when the name does not start with a
/// digit run followed by `_`.
fn parse_version(file_name: &str) -> Option<u64> {
    let digits: String = file_name
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() || !file_name[digits.len()..].starts_with('_') {
        return None;
    }
    digits.parse().ok()
}

/// Every `*.sql` file in `migrations/`, grouped by parsed version.
fn versions_on_disk(dir: &Path) -> BTreeMap<u64, Vec<String>> {
    let mut by_version: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    let entries = std::fs::read_dir(dir).expect("migrations/ must be readable");
    for entry in entries {
        let entry = entry.expect("migrations/ entry must be readable");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sql") {
            continue;
        }
        let version = parse_version(&name).unwrap_or_else(|| {
            panic!(
                "migration file {name:?} does not start with a numeric version prefix followed \
                 by '_'; sqlx cannot order it, so it must be renamed"
            )
        });
        by_version.entry(version).or_default().push(name);
    }
    for files in by_version.values_mut() {
        files.sort();
    }
    by_version
}

/// Groups of two or more files sharing one version, as `(version, files)`.
fn duplicate_versions(dir: &Path) -> Vec<(u64, Vec<String>)> {
    versions_on_disk(dir)
        .into_iter()
        .filter(|(_, files)| files.len() > 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A duplicate migration version must fail the build, not the relay.
    ///
    /// The failure message names every colliding file so the author of the
    /// second one knows exactly what to renumber.
    #[test]
    fn migration_versions_are_unique_on_disk() {
        let dir = migrations_dir();
        let duplicates = duplicate_versions(&dir);
        assert!(
            duplicates.is_empty(),
            "duplicate migration version prefixes in {}: {}. sqlx has no error for this: the \
             collision would otherwise surface at relay startup inside run_migrations, while it \
             holds the exclusive SCHEMA_DESTRUCTION_LOCK_KEY on a half-migrated schema. Renumber \
             the file that landed second to the next free version.",
            dir.display(),
            duplicates
                .iter()
                .map(|(version, files)| format!("{version:04} -> [{}]", files.join(", ")))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    /// sqlx's own resolver must see exactly the files we see.
    ///
    /// Independent of the uniqueness check, and of our parser: it walks the same
    /// directory through `MigrationSource`, which is the code path `migrate!`
    /// uses. If sqlx ever starts deduping colliding versions internally, its
    /// count drops below the on-disk file count and this fails; if our own
    /// prefix parser ever disagrees with sqlx's, the multisets differ and this
    /// fails. No database connection is opened.
    #[tokio::test]
    async fn sqlx_resolver_agrees_with_disk() {
        use sqlx::migrate::MigrationSource;

        let dir = migrations_dir();
        let disk = versions_on_disk(&dir);
        let disk_file_count: usize = disk.values().map(Vec::len).sum();

        let resolved = dir
            .as_path()
            .resolve()
            .await
            .expect("sqlx must resolve the migrations directory");
        let mut resolved_versions: Vec<i64> = resolved.iter().map(|m| m.version).collect();
        resolved_versions.sort_unstable();

        assert_eq!(
            resolved_versions.len(),
            disk_file_count,
            "sqlx resolved {} migrations but {} *.sql files exist in {}; resolved versions {:?}",
            resolved_versions.len(),
            disk_file_count,
            dir.display(),
            resolved_versions
        );

        let mut disk_versions: Vec<i64> = disk
            .iter()
            .flat_map(|(version, files)| files.iter().map(move |_| *version as i64))
            .collect();
        disk_versions.sort_unstable();

        assert_eq!(
            resolved_versions,
            disk_versions,
            "sqlx's resolved migration versions differ from the on-disk version multiset in {}",
            dir.display()
        );
    }

    /// The parser accepts real names and rejects the shapes sqlx cannot order.
    #[test]
    fn version_prefix_parsing() {
        assert_eq!(parse_version("0049_task_event_changes.sql"), Some(49));
        assert_eq!(parse_version("0001_initial.sql"), Some(1));
        assert_eq!(parse_version("20260925_dated_style.sql"), Some(20260925));
        assert_eq!(parse_version("no_prefix.sql"), None);
        assert_eq!(parse_version("0049-dash-separated.sql"), None);
        assert_eq!(parse_version(".sql"), None);
    }

    /// The duplicate detector reports collisions rather than the whole set.
    ///
    /// Uses a hand-rolled scratch directory instead of a `tempfile` dev-dependency
    /// so this item adds no dependency and touches no manifest.
    #[test]
    fn duplicate_detection_is_exact() {
        let scratch = std::env::temp_dir().join(format!(
            "buzz-migration-inventory-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        for name in [
            "0001_a.sql",
            "0002_b.sql",
            "0002_c.sql",
            "0003_d.sql",
            "README.md",
        ] {
            std::fs::write(scratch.join(name), "-- fixture\n").expect("write fixture");
        }
        let duplicates = duplicate_versions(&scratch);
        std::fs::remove_dir_all(&scratch).expect("remove scratch dir");
        assert_eq!(
            duplicates,
            vec![(2, vec!["0002_b.sql".to_string(), "0002_c.sql".to_string()])]
        );
    }
}
