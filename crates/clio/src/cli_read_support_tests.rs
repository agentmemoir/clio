// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared test fixtures for the CLI read-command
//! suites: injected-stream runners, JSON parsing, and in-process seed helpers.
//!
//! ## Owns
//! - `run_state`/`run_open` runners over `run_with_state` and `run_verb`
//! - In-process seeding helpers for items, tasks, failures, triples, beliefs,
//!   and graph links
//!
//! ## Does not own
//! - The commands under test (see the `cli_read_*` modules)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read::{CliIo, run_verb, run_with_state};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::process::ExitCode;
use std::sync::Arc;

/// A timestamp used by every seed helper.
pub(crate) const TS: &str = "2024-01-01T00:00:00Z";

pub(crate) fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf8")
}

/// Run a canonical verb against injected state (no database open).
pub(crate) fn run_state(
    verb: &str,
    args: &[&str],
    state: &Arc<McpState>,
    is_tty: bool,
) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env: &env,
        };
        run_with_state(verb, tokens(args), state, &mut io)
    };
    (code, utf8(out), utf8(err))
}

/// Run a top-level command through `run_verb` (resolves groups, opens state).
pub(crate) fn run_open(cmd: &str, args: &[&str], is_tty: bool) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env: &env,
        };
        run_verb(cmd, tokens(args), &mut io)
    };
    (code, utf8(out), utf8(err))
}

pub(crate) fn json_of(text: &str) -> Value {
    serde_json::from_str(text.trim()).expect("json output")
}

/// Open in-memory state with no deployment overlay.
pub(crate) fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

/// Dispatch an in-process tool call (the same seam the CLI uses).
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn call(state: &McpState, tool: &str, args: Value) -> Value {
    clio_mcp::write_tools::dispatch(state, tool, &args)
}

/// Store one admitted item and register its lexical doc.
pub(crate) fn seed_item(state: &McpState, bank: &str, id: &str, gist: &str) {
    let args = json!({
        "item": {
            "id": id,
            "subject_id": "s",
            "gist": gist,
            "snapshot": {"entity": gist, "amount": 1, "date": "2024-01-01"}
        },
        "category": "task_spec",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers {gist} (1 note since 2024-01-01)."),
        "bank": bank,
        "actor": "agent",
        "timestamp": TS
    });
    let stored = call(state, "store", args);
    assert_eq!(stored["ok"], json!(true), "seed {id}: {stored}");
    state
        .index
        .put_lexical_doc(bank, id, gist)
        .expect("lexical doc");
}

pub(crate) fn seed_task(state: &McpState, bank: &str, task_id: &str, definition: &str) {
    let stored = call(
        state,
        "task_upsert",
        json!({
            "bank": bank, "task_id": task_id, "definition": definition,
            "status": "running", "timestamp": TS
        }),
    );
    assert_eq!(stored["ok"], json!(true), "task {task_id}: {stored}");
}

pub(crate) fn seed_failure(
    state: &McpState,
    bank: &str,
    task_id: &str,
    attempt: i64,
    lesson: &str,
) {
    let stored = call(
        state,
        "failure_record",
        json!({
            "bank": bank, "task_id": task_id, "attempt_n": attempt,
            "what_failed": "flake", "lesson": lesson, "timestamp": TS
        }),
    );
    assert_eq!(stored["ok"], json!(true), "failure {task_id}: {stored}");
}

pub(crate) fn seed_triple(
    state: &McpState,
    bank: &str,
    subject: &str,
    predicate: &str,
    object: &str,
) {
    let stored = call(
        state,
        "triple_add",
        json!({
            "bank": bank, "subject": subject, "predicate": predicate,
            "object": object, "epistemic_kind": "fact", "timestamp": TS
        }),
    );
    assert_eq!(stored["ok"], json!(true), "triple {subject}: {stored}");
}

pub(crate) fn seed_belief(state: &McpState, bank: &str, proposition: &str, confidence: f64) {
    let stored = call(
        state,
        "belief_observe",
        json!({
            "bank": bank, "proposition": proposition, "confidence": confidence,
            "evidence_ref": "ev-1", "source_type": "user_stated", "timestamp": TS
        }),
    );
    assert_eq!(stored["ok"], json!(true), "belief: {stored}");
}

pub(crate) fn seed_link(state: &McpState, bank: &str, source: &str, target: &str) {
    let linked = call(
        state,
        "graph_link",
        json!({
            "bank": bank, "source_id": source, "target_id": target,
            "relationship": "relates_to"
        }),
    );
    assert_eq!(linked["ok"], json!(true), "link: {linked}");
}
