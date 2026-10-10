// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared test fixtures for the CLI write-command
//! suites: injected-stream runners over the write engine and the read engine's
//! group resolution seam.
//!
//! ## Owns
//! - `run_write_state` / `run_write_open` runners for the write surface
//!
//! ## Does not own
//! - The commands under test (see the `cli_write_*` modules)
//! - The read-suite helpers (`state`, `call`, `json_of`, `tokens`) it reuses
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read::{CliIo, run_verb};
use crate::cli_read_test_support::{TS, call, state};
use crate::cli_write::run_with_state;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf8")
}

/// Two admitted items in `demo` so graph endpoints exist.
pub(crate) fn link_state() -> (Arc<clio_mcp::runtime::McpState>, String, String) {
    let s = state();
    for id in ["itm-a", "itm-b"] {
        let stored = call(
            &s,
            "store",
            json!({
                "item": {"id": id, "subject_id": "s", "gist": format!("prefers {id}")},
                "category": "persona", "epistemic_kind": "fact",
                "bank": "demo", "timestamp": TS
            }),
        );
        assert_eq!(stored["ok"], json!(true), "{stored}");
    }
    (s, "itm-a".into(), "itm-b".into())
}

/// Run a resolved write verb against injected state (no database open).
pub(crate) fn run_write_state(
    verb: &str,
    args: &[&str],
    state: &Arc<clio_mcp::runtime::McpState>,
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
        run_with_state(
            verb,
            crate::cli_read_test_support::tokens(args),
            state,
            &mut io,
        )
    };
    (code, utf8(out), utf8(err))
}

/// Run a resolved write verb against injected state with injected stdin
/// (for `batch --file -`); returns the exit code and both stream buffers.
pub(crate) fn run_write_state_stdin(
    verb: &str,
    args: &[&str],
    stdin: &mut dyn std::io::Read,
    state: &Arc<clio_mcp::runtime::McpState>,
    is_tty: bool,
) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin,
            is_tty,
            no_color: true,
            env: &env,
        };
        run_with_state(
            verb,
            crate::cli_read_test_support::tokens(args),
            state,
            &mut io,
        )
    };
    (code, utf8(out), utf8(err))
}

/// Run a resolved write verb through the database-opening `run_resolved` seam
/// with injected streams. Only used for parse/help paths that return before a
/// store is opened, so no database is touched.
pub(crate) fn run_write_resolved(verb: &str, args: &[&str]) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        crate::cli_write::run_resolved(verb, crate::cli_read_test_support::tokens(args), &mut io)
    };
    (code, utf8(out), utf8(err))
}

/// Run a top-level command through the read engine's group resolution (which
/// delegates write verbs to the write engine and opens state).
pub(crate) fn run_write_open(cmd: &str, args: &[&str], is_tty: bool) -> (ExitCode, String, String) {
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
        run_verb(cmd, crate::cli_read_test_support::tokens(args), &mut io)
    };
    (code, utf8(out), utf8(err))
}

/// State with the shared-bank surface enabled (the CLI's `--shared-bank`
/// opt-in) so the shared verbs pass the deployment gate in tests.
pub(crate) fn shared_state() -> Arc<clio_mcp::runtime::McpState> {
    clio_mcp::runtime::McpState::open_with_effective(
        &clio_mcp::runtime::McpOpenOptions {
            shared_bank_enabled: true,
            ..clio_mcp::runtime::McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect("shared state")
}
