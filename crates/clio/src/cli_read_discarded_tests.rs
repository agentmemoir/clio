// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` read surface's JSON report of a discarded item,
//! driven through the real discard write path rather than a hand-built payload.
//!
//! ## Owns
//! - `discarded: true` on the `get` and `show` spellings of the item read
//!
//! ## Does not own
//! - The text view of the same flags (`cli_read_render_get_tests`)
//! - Argument decoding, output-mode selection, or the tool payload
//!   (`cli_read`, `read_tools`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the discarded item's JSON report.

use super::*;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};

fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf8")
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text.trim()).expect("json output")
}

fn state_with_one_item() -> Arc<McpState> {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    let stored = clio_mcp::write_tools::dispatch(
        &state,
        "store",
        &json!({
            "item": {
                "id": "itm-1",
                "subject_id": "s",
                "gist": "terse answers",
                "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
            },
            "category": "task_spec",
            "epistemic_kind": "fact",
            "source_text": "The user prefers terse answers (1 note since 2024-01-01).",
            "bank": "bank-a",
            "actor": "agent",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");
    state
}

fn run_state(verb: &str, args: &[&str], state: &Arc<McpState>) -> (ExitCode, String, String) {
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
        run_with_state(verb, tokens(args), state, &mut io)
    };
    (code, utf8(out), utf8(err))
}

#[test]
fn a_discarded_item_reports_discarded_on_get_and_show() {
    let state = state_with_one_item();
    let (code, out, _) = run_state("get", &["itm-1", "--bank", "bank-a"], &state);
    assert_eq!(code, ExitCode::from(0));
    assert_eq!(json_of(&out)["discarded"], json!(false), "{out}");

    let discarded = clio_mcp::write_tools::dispatch(
        &state,
        "discard",
        &json!({
            "bank": "bank-a",
            "item_id": "itm-1",
            "reason": "ops",
            "confirm": true,
            "timestamp": "2024-02-01T00:00:00Z"
        }),
    );
    assert_eq!(discarded["ok"], json!(true), "{discarded}");

    for verb in ["get", "show"] {
        let (code, out, err) = run_state(verb, &["itm-1", "--bank", "bank-a"], &state);
        assert_eq!(code, ExitCode::from(0), "{verb}: {err}");
        let value = json_of(&out);
        assert_eq!(value["discarded"], json!(true), "{verb}: {out}");
        assert_eq!(value["archived"], json!(false), "{verb}: {out}");
    }
}
