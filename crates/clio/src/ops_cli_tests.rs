// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio ops` CLI surface: exit-code mapping for each
//! operator subcommand, the backend/flag handling around them, and the
//! persistent key-store resolution `reindex` needs to rewrite real rows.
//!
//! ## Owns
//! - Exit-code tests for `diagnose`, `verify`, `doctor`, `repair`, `reindex`
//! - Unknown-subcommand and unknown-backend tests
//! - The key-store split regressions: reindex and repair resolve the sibling
//!   KMS, read-only commands answer without creating one
//!
//! ## Does not own
//! - The `clio ops` implementation under test (`ops_cli`)
//! - The failing-verify fault test (`ops_cli_fault_tests`)
//! - Operator check semantics (`clio-ops`)
//!
//! ## Boundary
//!
//! Tests here break when the `clio ops` exit-code contract changes; they never
//! change it.
//!
//! Keep this module focused on the `clio ops` command surface.

use super::*;

/// An environment lookup that reports nothing set, which is what "this machine
/// has no deployment config" means (#90).
fn no_env(key: &str) -> Option<String> {
    let _ = key;
    None
}

/// `clio ops` with the environment supplied by the caller rather than read from
/// the process, so a verdict here is about the code and not about the host.
///
/// This replaces the `crate::run` call these tests used. `run` reads the real
/// environment on the way in, which is what let a developer's configured
/// sidecar decide whether `reindex --target dense` refused or succeeded (#90).
fn run_ops(parts: &[&str]) -> ExitCode {
    run_ops_args(parts.iter().map(|p| (*p).to_owned()))
}

/// The same, for a test that builds its own argument vector.
///
/// Generic so a test may pass either a `Vec` or a fixed-size array; `ops_with`
/// takes an iterator, so both already fit.
///
/// **No `ops` prefix.** `ops_with` is the body of the `ops` subcommand, and
/// `main` strips the word `ops` before calling it, so the first argument it
/// reads is the subcommand itself — `diagnose`, not `ops diagnose`. Passing the
/// prefix here made every test in this file report an unknown command named
/// "ops" and exit 2, which is 14 failures that look like a product regression
/// and are not one.
fn run_ops_args(args: impl IntoIterator<Item = String>) -> ExitCode {
    super::ops_with(args, &no_env)
}

fn memory(sub: &str) -> Vec<String> {
    vec![
        sub.to_owned(),
        "--backend".to_owned(),
        "sqlite".to_owned(),
        "--db".to_owned(),
        "sqlite::memory:".to_owned(),
    ]
}

#[test]
fn diagnose_memory_exits_0() {
    assert_eq!(run_ops_args(memory("diagnose")), ExitCode::SUCCESS);
}

#[test]
fn verify_empty_store_exits_0() {
    assert_eq!(run_ops_args(memory("verify")), ExitCode::SUCCESS);
}

#[test]
fn doctor_memory_exits_0() {
    assert_eq!(run_ops_args(memory("doctor")), ExitCode::SUCCESS);
}

#[test]
fn doctor_live_registers_plan_exits_0() {
    let mut args = memory("doctor");
    args.extend(["--dry-run", "false"].map(str::to_owned));
    assert_eq!(run_ops_args(args), ExitCode::SUCCESS);
}

#[test]
fn repair_without_confirm_refused_exits_2() {
    let mut args = memory("repair");
    args.extend(["--actions", "drop_orphan_assoc_edge"].map(str::to_owned));
    assert_eq!(run_ops_args(args), ExitCode::from(2));
}

#[test]
fn repair_unknown_action_exits_2() {
    let mut args = memory("repair");
    args.extend(["--actions", "nope", "--confirm"].map(str::to_owned));
    assert_eq!(run_ops_args(args), ExitCode::from(2));
}

#[test]
fn repair_missing_plan_exits_2() {
    let mut args = memory("repair");
    args.extend(["--plan", "plan-1", "--confirm"].map(str::to_owned));
    assert_eq!(run_ops_args(args), ExitCode::from(2));
}

#[test]
fn repair_confirmed_drop_orphans_exits_0() {
    let mut args = memory("repair");
    args.extend(
        [
            "--bank",
            "default",
            "--actions",
            "drop_orphan_assoc_edge",
            "--confirm",
        ]
        .map(str::to_owned),
    );
    assert_eq!(run_ops_args(args), ExitCode::SUCCESS);
}

#[test]
fn missing_ops_subcommand_exits_2() {
    assert_eq!(run_ops(&[]), ExitCode::from(2));
}

#[test]
fn reindex_dry_run_exits_0() {
    let mut args = memory("reindex");
    args.push("--dry-run".to_owned());
    assert_eq!(run_ops_args(args), ExitCode::SUCCESS);
}

#[test]
fn reindex_defaults_to_dry_run_exits_0() {
    assert_eq!(run_ops_args(memory("reindex")), ExitCode::SUCCESS);
}

#[test]
fn reindex_dense_live_unconfirmed_exits_3() {
    let mut args = memory("reindex");
    args.extend(["--target", "dense", "--dry-run", "false"].map(str::to_owned));
    assert_eq!(run_ops_args(args), ExitCode::from(3));
}

#[test]
fn reindex_dense_confirmed_without_embedder_exits_3() {
    let mut args = memory("reindex");
    args.extend(["--target", "dense", "--dry-run", "false", "--confirm"].map(str::to_owned));
    assert_eq!(run_ops_args(args), ExitCode::from(3));
}

#[test]
fn reindex_bad_target_exits_3() {
    let mut args = memory("reindex");
    args.extend(["--target", "bogus"].map(str::to_owned));
    assert_eq!(run_ops_args(args), ExitCode::from(3));
}

/// A regression test for the reindex key store: the command must read the
/// persistent sibling key store, not an empty process-local KMS, or every item
/// is unreadable and the sweep rewrites zero rows while still exiting 0.
#[test]
fn reindex_lexical_rewrites_rows_through_the_sibling_key_store() {
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-reindex-kms");
    let db = scratch.join("clio.db");
    crate::backup_cli_fixtures::populated_store(&db, 3, 1);
    // The writer opened the sibling key store; the CLI must resolve the same
    // one from `--db`, so the item content stays readable to the sweep.
    let args = [
        "reindex".to_owned(),
        "--backend".to_owned(),
        "sqlite".to_owned(),
        "--db".to_owned(),
        db.display().to_string(),
        "--target".to_owned(),
        "lexical".to_owned(),
        "--dry-run".to_owned(),
        "false".to_owned(),
        "--confirm".to_owned(),
    ];
    assert_eq!(run_ops_args(args), ExitCode::SUCCESS);

    let kms: Arc<dyn DekProvider> = Arc::new(
        LocalDevKms::open_persistent(crate::backup_cli_fixtures::key_store_path(&db))
            .expect("open the sibling key store"),
    );
    let store = SqliteStore::open(&db, kms).expect("reopen the store");
    let cov = IndexStore::index_coverage(&store, None).expect("coverage");
    assert_eq!(cov.indexable_items, 3);
    assert_eq!(cov.lexical_rows, 3, "every item row was rewritten");
    let hits = store
        .lexical_search("demo", "planted", &[], 10)
        .expect("search after the CLI reindex");
    assert_eq!(
        hits.len(),
        3,
        "the rewritten rows match a query under the persistent key"
    );
}

#[test]
fn unknown_ops_subcommand_exits_2() {
    assert_eq!(run_ops(&["bogus"]), ExitCode::from(2));
}

/// The repair arm of the key-store split: `rebuild_lexical_index` re-derives
/// lexical docs from sealed content, so it must read the persistent sibling
/// key store exactly like `reindex`. An empty process-local KMS would make
/// every item unreadable and the repair would report success while rewriting
/// nothing.
#[test]
fn repair_rebuild_lexical_rewrites_rows_through_the_sibling_key_store() {
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-repair-kms");
    let db = scratch.join("clio.db");
    crate::backup_cli_fixtures::populated_store(&db, 3, 1);
    let args = [
        "repair".to_owned(),
        "--backend".to_owned(),
        "sqlite".to_owned(),
        "--db".to_owned(),
        db.display().to_string(),
        "--actions".to_owned(),
        "rebuild_lexical_index".to_owned(),
        "--bank".to_owned(),
        "demo".to_owned(),
        "--confirm".to_owned(),
    ];
    assert_eq!(run_ops_args(args), ExitCode::SUCCESS);

    let kms: Arc<dyn DekProvider> = Arc::new(
        LocalDevKms::open_persistent(crate::backup_cli_fixtures::key_store_path(&db))
            .expect("open the sibling key store"),
    );
    let store = SqliteStore::open(&db, kms).expect("reopen the store");
    let cov = IndexStore::index_coverage(&store, None).expect("coverage");
    assert_eq!(cov.indexable_items, 3);
    assert_eq!(cov.lexical_rows, 3, "every item row was rebuilt");
    let hits = store
        .lexical_search("demo", "planted", &[], 10)
        .expect("search after the CLI repair");
    assert_eq!(
        hits.len(),
        3,
        "the rebuilt rows match a query under the persistent key"
    );
}

/// The read-only side of the same split: `diagnose` must answer on a store
/// whose key-store file is gone, and must not recreate one. (The store was
/// populated with its own key store, then that file is removed.)
#[test]
fn diagnose_on_a_store_whose_key_store_is_gone_answers_without_creating_it() {
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-diagnose-kms");
    let db = scratch.join("clio.db");
    crate::backup_cli_fixtures::populated_store(&db, 2, 1);
    let kms_path = crate::backup_cli_fixtures::key_store_path(&db);
    assert!(kms_path.exists(), "the populated store has a key store");
    std::fs::remove_file(&kms_path).expect("remove the key store");

    let args = [
        "diagnose".to_owned(),
        "--backend".to_owned(),
        "sqlite".to_owned(),
        "--db".to_owned(),
        db.display().to_string(),
    ];
    assert_eq!(run_ops_args(args), ExitCode::SUCCESS);
    assert!(
        !kms_path.exists(),
        "a read-only ops command must not create a key store"
    );
}

#[test]
fn unknown_backend_exits_1() {
    assert_eq!(
        run_ops(&["diagnose", "--backend", "mysql"]),
        ExitCode::from(1)
    );
}

#[test]
fn ops_help_exits_0() {
    assert_eq!(run_ops(&["--help"]), ExitCode::SUCCESS);
    assert_eq!(run_ops(&["diagnose", "--help"]), ExitCode::SUCCESS);
}

#[test]
fn an_undeclared_ops_flag_is_a_usage_error() {
    assert_eq!(
        run_ops(&["diagnose", "--bogus"]),
        ExitCode::from(2),
        "an unknown flag fails closed instead of running with defaults"
    );
}

/// A flag a sibling subcommand owns is refused here, not silently ignored: the
/// flag lists are per subcommand, not one union over the whole surface.
#[test]
fn a_flag_from_another_ops_subcommand_is_a_usage_error() {
    assert_eq!(
        run_ops(&["diagnose", "--plan", "p1"]),
        ExitCode::from(2),
        "diagnose does not take repair's --plan"
    );
}

/// The subcommand that owns each flag still accepts it.
#[test]
fn each_ops_subcommand_accepts_its_own_flags() {
    let flags = crate::cli_top_level::parse_flags(
        ["--plan".to_owned(), "p1".to_owned()],
        super::ops_flags("repair"),
    )
    .expect("repair takes --plan");
    assert_eq!(flags.get("plan").map(String::as_str), Some("p1"));

    let error = crate::cli_top_level::parse_flags(
        ["--plan".to_owned(), "p1".to_owned()],
        super::ops_flags("diagnose"),
    )
    .expect_err("diagnose does not take --plan");
    assert_eq!(error.code(), "usage");
}
