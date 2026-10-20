// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the local event-review route of one
//! deployment: that `clio compliance request-events` reads back the event rows
//! the request itself wrote, over a real store, through the real binary.
//!
//! ## Owns
//! - The event-review case on SQLite and on Postgres
//! - The assertions that the page is not empty and carries the request's own row
//!
//! ## Does not own
//! - The CLI driver and the request deployment (see `cli_test_support` and
//!   `refusals_test_support`)
//! - The seeded stores and the Postgres gate (see `case_fixtures` and
//!   `support_pg_test_support`)
//! - The event contract and the request state machine (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to which route the event review is proven through should normally
//! require changes here. Changes to the event contract should not.
//!
//! Keep this module focused on the event-review claim.

use crate::case_fixtures::{
    BANK, DENIED_ITEM, DENIED_MARKER, DENIED_SUBJECT, seed_pg, seed_sqlite,
};
use crate::cli_test_support::{clio_command, run_captured};
use crate::refusals_test_support::Env;
use crate::support_pg_test_support::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::store::ScopedSchema;
use clio_compliance::ProtectedAction;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The subject the case erases and then reviews.
const SUBJECT: &str = DENIED_SUBJECT;

/// The Clio encryption key the in-process fixtures and the `clio` children
/// share, so a key store one writer seals the other can open.
const KEY: [u8; 32] = [0x3c; 32];

/// The action id the erasure producer derives from the request id.
///
/// The producer's own id is the source, so the assertion names the row the
/// production writes instead of a spelling this test invents.
fn erasure_action_id(request: &str) -> String {
    format!("{}:{request}", ProtectedAction::Erasure.producer_id())
}

/// The thread-local key this scenario installs, cleared when the guard drops.
///
/// The key is a thread-local, so a scenario that leaves it installed hands it
/// to the next test the harness runs on the same thread.
struct ThreadKeyGuard;

impl Drop for ThreadKeyGuard {
    fn drop(&mut self) {
        clio_config::secret_seal::test_key::clear_thread_key();
    }
}

/// Install the shared Clio encryption key on this thread and write it to the
/// file the `clio` children open.
///
/// The returned guard clears the thread-local key when it drops, so the key
/// never outlives the scenario that installed it.
fn install_key(scratch: &Path) -> (PathBuf, ThreadKeyGuard) {
    clio_config::secret_seal::test_key::use_key(KEY);
    let path = scratch.join("clio.key");
    std::fs::write(&path, KEY).expect("write the scenario key file");
    (path, ThreadKeyGuard)
}

/// Run one `compliance` request verb against the deployment and read its JSON
/// payload.
///
/// The run is the bounded one the shared CLI driver owns: the child is killed
/// and reaped at its deadline, and both streams are drained.
fn run(env: &Env, key: &Path, verb: &str, args: &[&str]) -> Value {
    let mut all = vec!["compliance", verb];
    all.extend_from_slice(args);
    all.extend(["--db", env.db(), "-o", "json"]);
    let mut command = clio_command(&env.journal, &env.kms, &env.backups);
    command
        .env("CLIO_KEY_FILE", key)
        .env("CLIO_KEY_FILE_ONLY", "1")
        .args(&all);
    let out = run_captured(&mut command);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the verb failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|error| {
        panic!(
            "the payload is not JSON ({error}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The event-review route reads the rows the request wrote.
///
/// The request is opened and its erasure is confirmed, so the request writes
/// its own event row. The event-review route then reads that row back: the
/// page is not empty, and it carries the request's own action, settled.
fn event_review(env: &Env, key: &Path, request: &str) {
    let started = run(
        env,
        key,
        "request-start",
        &[
            "--request-id",
            request,
            "--kind",
            "erasure",
            "--subject-id",
            SUBJECT,
            "--bank",
            BANK,
            "--actor",
            "harness",
        ],
    );
    assert_eq!(started["request_id"], json!(request), "{started}");
    let retried = run(
        env,
        key,
        "request-retry",
        &[
            "--request-id",
            request,
            "--subject-id",
            SUBJECT,
            "--legal-basis",
            "GDPR_ART_17",
            "--confirm",
            "--bank",
            BANK,
            "--actor",
            "harness",
        ],
    );
    assert_eq!(retried["request_id"], json!(request), "{retried}");

    let page = run(
        env,
        key,
        "request-events",
        &[
            "--request-id",
            request,
            "--limit",
            "50",
            "--bank",
            BANK,
            "--actor",
            "harness",
        ],
    );
    let rows = page["rows"].as_array().expect("the event rows");
    assert!(!rows.is_empty(), "the page is not empty: {page}");
    let action_id = erasure_action_id(request);
    let own: Vec<&Value> = rows
        .iter()
        .filter(|row| row["action_id"] == json!(action_id))
        .collect();
    assert_eq!(own.len(), 1, "the request's own row is in the page: {page}");
    assert_eq!(
        own[0]["state"],
        json!("allowed"),
        "the request's own row is settled: {page}"
    );
    assert_eq!(
        page["has_more"],
        json!(false),
        "one page holds every row: {page}"
    );
    assert_eq!(page["next_cursor"], Value::Null, "no next page: {page}");
}

/// The event-review route reads the request's own rows back on SQLite.
#[test]
fn scenario_event_review_sqlite_reads_the_rows_the_request_wrote() {
    let env = Env::new("event-review");
    let (key, _key_guard) = install_key(&env.scratch);
    seed_sqlite(
        env.path(),
        &env.kms,
        &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)],
    );
    event_review(&env, &key, "req-event-review");
}

/// The same event-review route on loopback Postgres.
#[test]
fn scenario_event_review_postgres_reads_the_rows_the_request_wrote() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let target = ScopedSchema::new(&base);
    let env = Env::postgres("event-review-pg", target.url());
    let (key, _key_guard) = install_key(&env.scratch);
    seed_pg(
        env.db(),
        &env.kms,
        &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)],
    );
    event_review(&env, &key, "req-event-review-pg");
}
