// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the three mutating `clio ops` handlers that
//! `ops_cli_actions` owns: `doctor`, `repair`, and `reindex`, including the
//! exit code each outcome maps to and the flag rules the handlers read.
//!
//! ## Owns
//! - Exit-code tests for the `doctor`, `repair`, and `reindex` handlers
//! - The `--actions` list parsing and the `--confirm` value rules the handlers
//!   apply
//! - The usage-error path for a bad `reindex --target` value
//!
//! ## Does not own
//! - The handlers under test (`ops_cli_actions`)
//! - The `clio ops` dispatcher and backend resolution (`ops_cli`, covered by
//!   `ops_cli_tests`)
//! - The doctor, repair, and reindex semantics (`clio-ops`)
//!
//! ## Boundary
//!
//! Tests here break when a handler maps an outcome to a different exit code or
//! reads a flag differently; they never change that contract.

use super::*;
use clio_store::{DekProvider, LocalDevKms, SqliteStore};
use std::sync::Arc;

/// One timestamp a test fixes, so no verdict depends on the host clock.
const TIMESTAMP: &str = "2026-01-01T00:00:00Z";

/// An in-memory store with a process-local key store: the shape a handler
/// receives after the dispatcher opens the backend.
fn memory_store() -> SqliteStore {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    SqliteStore::open_in_memory(kms).expect("open the in-memory store")
}

/// The flags one test passes, built from pairs so each test states its input.
fn flags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// A fresh process-local plan registry, like the dispatcher builds.
fn plans() -> clio_ops::PlanRegistry {
    clio_ops::PlanRegistry::new()
}

/// A clean dry-run `doctor` composes a plan with no findings and exits 0.
#[test]
fn doctor_dry_run_on_a_clean_store_exits_0() {
    let store = memory_store();
    let code = run_doctor(&store, None, &flags(&[]), TIMESTAMP, &plans());
    assert_eq!(code, ExitCode::SUCCESS);
}

/// A live `doctor` run records the plan instead of only composing it, and a
/// clean store still exits 0.
#[test]
fn doctor_live_run_records_the_plan_exits_0() {
    let store = memory_store();
    let code = run_doctor(
        &store,
        None,
        &flags(&[("dry-run", "false")]),
        TIMESTAMP,
        &plans(),
    );
    assert_eq!(code, ExitCode::SUCCESS);
}

/// `repair` without `--confirm` refuses before any mutation: exit 2.
#[test]
fn repair_without_confirm_refuses_exits_2() {
    let store = memory_store();
    let code = run_repair(
        &store,
        None,
        &flags(&[("actions", "drop_orphan_assoc_edge")]),
        TIMESTAMP,
        &plans(),
        None,
    );
    assert_eq!(code, ExitCode::from(2));
}

/// An action the allowlist does not know is refused: exit 2.
#[test]
fn repair_unknown_action_exits_2() {
    let store = memory_store();
    let code = run_repair(
        &store,
        Some("default"),
        &flags(&[("actions", "nope"), ("confirm", "true")]),
        TIMESTAMP,
        &plans(),
        None,
    );
    assert_eq!(code, ExitCode::from(2));
}

/// `--actions` entries are trimmed, and empty entries between commas are
/// dropped: `" , drop_orphan_assoc_edge , "` is one known action, so a
/// confirmed clean repair applies it and exits 0.
#[test]
fn repair_trims_and_drops_empty_action_entries_exits_0() {
    let store = memory_store();
    let code = run_repair(
        &store,
        Some("default"),
        &flags(&[
            ("actions", " , drop_orphan_assoc_edge , "),
            ("confirm", "true"),
        ]),
        TIMESTAMP,
        &plans(),
        None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
}

/// No `--actions` at all leaves an empty list, and a confirmed repair with no
/// plan and no actions refuses: exit 2.
#[test]
fn repair_with_no_actions_and_no_plan_exits_2() {
    let store = memory_store();
    let code = run_repair(
        &store,
        None,
        &flags(&[("confirm", "true")]),
        TIMESTAMP,
        &plans(),
        None,
    );
    assert_eq!(code, ExitCode::from(2));
}

/// A plan id the registry does not hold fails with `PLAN_NOT_FOUND`: exit 1.
#[test]
fn repair_missing_plan_exits_1() {
    let store = memory_store();
    let code = run_repair(
        &store,
        None,
        &flags(&[("plan", "plan-1"), ("confirm", "true")]),
        TIMESTAMP,
        &plans(),
        None,
    );
    assert_eq!(code, ExitCode::from(1));
}

/// No flags is a report-only rebuild: it applies nothing, so it is never `ok`
/// and exits 3.
#[test]
fn reindex_without_flags_reports_and_exits_3() {
    let store = memory_store();
    let code = run_reindex(&store, None, &flags(&[]), None);
    assert_eq!(code, ExitCode::from(3));
}

/// `--confirm false` is a value the handler honours, not a presence: it stays
/// a report and exits 3.
#[test]
fn reindex_confirm_false_reports_exits_3() {
    let store = memory_store();
    let code = run_reindex(&store, None, &flags(&[("confirm", "false")]), None);
    assert_eq!(code, ExitCode::from(3));
}

/// `--confirm` with an explicit `--dry-run true` asks for two opposite things,
/// so the rebuild is refused: exit 3.
#[test]
fn reindex_confirm_with_dry_run_true_is_refused_exits_3() {
    let store = memory_store();
    let code = run_reindex(
        &store,
        None,
        &flags(&[("confirm", "true"), ("dry-run", "true")]),
        None,
    );
    assert_eq!(code, ExitCode::from(3));
}

/// A live, confirmed lexical rebuild on a clean store applies its sweep and
/// exits 0.
#[test]
fn reindex_live_lexical_on_a_clean_store_exits_0() {
    let store = memory_store();
    let code = run_reindex(
        &store,
        None,
        &flags(&[("target", "lexical"), ("confirm", "true")]),
        None,
    );
    assert_eq!(code, ExitCode::SUCCESS);
}

/// A `--target` value that parses as no rebuild target is a usage error: exit
/// 2, before any store work.
#[test]
fn reindex_bad_target_is_a_usage_error_exits_2() {
    let store = memory_store();
    let code = run_reindex(&store, None, &flags(&[("target", "bogus")]), None);
    assert_eq!(code, ExitCode::from(2));
}
