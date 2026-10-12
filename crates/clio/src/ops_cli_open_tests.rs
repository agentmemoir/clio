// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the open paths of the `clio ops` CLI: the refusals that
//! happen before a subcommand runs (an unopenable configured address, an
//! unopenable SQLite file, an unreachable Postgres backend, an embedding-space
//! switch without the opt-in) and the five subcommands over a real Postgres
//! store.
//!
//! ## Owns
//! - The `run_backend` open-failure tests and the Postgres exit-code walk
//!
//! ## Does not own
//! - The `clio ops` implementation under test (`ops_cli`)
//! - The exit-code contract tests (`ops_cli_tests.rs`)
//! - The failing-verify fault test (`ops_cli_fault_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the open-time refusals or the Postgres exit codes
//! change; they never change them.
//!
//! Keep this module focused on the `clio ops` open paths.

use super::*;
use crate::pg_test_helpers::pg_base_url;
use clio_store::{EmbeddingSpace, ScopedSchema, SpaceReconcile, lock_pg_test};

/// A configured address this build cannot open (`redis://`) is refused with
/// exit 1 before any store is opened.
#[test]
fn an_unopenable_configured_address_exits_1() {
    let env = |key: &str| match key {
        "CLIO_DATABASE_URL" => Some("redis://127.0.0.1:6379/0".to_owned()),
        _ => None,
    };
    assert_eq!(
        super::ops_with(["diagnose".to_owned()], &env),
        ExitCode::from(1)
    );
}

/// A SQLite path that is a directory, not a database file, fails the open
/// with exit 1 instead of falling back to anything.
#[test]
fn a_sqlite_db_that_cannot_open_exits_1() {
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-sqlite-open-refused");
    let dir = scratch.join("not-a-db");
    std::fs::create_dir(&dir).expect("create the directory");
    let env = |_key: &str| None;
    let args = [
        "diagnose",
        "--backend",
        "sqlite",
        "--db",
        &dir.to_string_lossy(),
    ]
    .map(str::to_owned);
    assert_eq!(super::ops_with(args, &env), ExitCode::from(1));
}

/// A configured embedding space that differs from the store's recorded space
/// is refused at open time without the `embed.switch_space` opt-in: exit 1,
/// failed closed rather than purging the dense index.
#[test]
fn an_embedding_space_switch_without_confirm_refuses_sqlite_exits_1() {
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-space-refusal");
    let db = scratch.join("clio.db");
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let mut store = SqliteStore::open(&db, Arc::clone(&kms)).expect("seed store");
    store
        .reconcile_space(Some(&EmbeddingSpace::default_space()), true)
        .expect("record the seed space");
    drop(store);
    let env = |key: &str| match key {
        "CLIO_EMBED_MODEL" => Some("other/model".to_owned()),
        _ => None,
    };
    let args = [
        "diagnose".to_owned(),
        "--backend".to_owned(),
        "sqlite".to_owned(),
        "--db".to_owned(),
        db.display().to_string(),
    ];
    assert_eq!(super::ops_with(args, &env), ExitCode::from(1));
}

/// An unreachable Postgres backend reports the open failure with exit 1.
///
/// The port comes from a bound-then-dropped loopback listener, so the
/// refusal is immediate and the test assumes no fixed port.
#[test]
fn unreachable_postgres_backend_exits_1() {
    let probe =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral loopback port");
    let port = probe.local_addr().expect("read back the bound port").port();
    drop(probe);
    let url = format!("postgres://clio:clio@127.0.0.1:{port}/nosuchdb");
    let env = |_key: &str| None;
    let args = ["diagnose", "--backend", "postgres", "--db", url.as_str()].map(str::to_owned);
    assert_eq!(super::ops_with(args, &env), ExitCode::from(1));
}

/// The five operator subcommands against a real Postgres store, each with its
/// documented exit code: diagnose, verify, doctor, and a dry-run reindex
/// succeed; the unconfirmed repair is refused with 2; and a mismatched
/// embedding space fails the postgres open closed with 1. Serialized on the
/// shared Postgres test lock, rows isolated in a scoped schema.
#[test]
fn postgres_ops_subcommands_exit_with_the_documented_codes() {
    let _guard = lock_pg_test();
    let scratch = crate::backup_cli_fixtures::Scratch::new("ops-postgres-ops");
    let kms_path = scratch.join("clio.kms");
    let scoped = ScopedSchema::new(&pg_base_url());
    let db = scoped.url().to_owned();
    // Record the default space on the fresh schema, so the walk below opens
    // against a recorded space and the final mismatched run is a refusal
    // rather than a silent switch on a fresh store.
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let mut seed = PostgresStore::open(&db, Arc::clone(&kms)).expect("open the schema's store");
    seed.reconcile_space(Some(&EmbeddingSpace::default_space()), true)
        .expect("record the seed space");
    drop(seed);
    let env = |key: &str| match key {
        "CLIO_KMS_FILE" => Some(kms_path.to_string_lossy().into_owned()),
        _ => None,
    };
    let postgres = |sub: &str| {
        vec![
            sub.to_owned(),
            "--backend".to_owned(),
            "postgres".to_owned(),
            "--db".to_owned(),
            db.clone(),
        ]
    };
    assert_eq!(
        super::ops_with(postgres("diagnose"), &env),
        ExitCode::SUCCESS
    );
    assert_eq!(super::ops_with(postgres("verify"), &env), ExitCode::SUCCESS);
    assert_eq!(super::ops_with(postgres("doctor"), &env), ExitCode::SUCCESS);
    let mut refused = postgres("repair");
    refused.extend(["--actions", "drop_orphan_assoc_edge"].map(str::to_owned));
    assert_eq!(super::ops_with(refused, &env), ExitCode::from(2));
    let mut dry = postgres("reindex");
    dry.push("--dry-run".to_owned());
    assert_eq!(super::ops_with(dry, &env), ExitCode::SUCCESS);

    let half_space = |key: &str| match key {
        "CLIO_EMBED_MODEL" => Some("other/model".to_owned()),
        _ => None,
    };
    assert_eq!(
        super::ops_with(postgres("diagnose"), &half_space),
        ExitCode::from(1)
    );
}
