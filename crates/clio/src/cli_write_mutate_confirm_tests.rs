// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the confirmed `clio correct` path through the global
//! `--yes` alias, including the supersession it records.
//!
//! ## Owns
//! - The `correct --yes` behavior test
//!
//! ## Does not own
//! - The commands under test (`cli_write_mutate`)
//! - The shared mutation fixtures and the confirm refusal (see the `tests` sibling)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{call, json_of, seed_item, state};
use crate::cli_write_test_support::run_write_state;
use serde_json::json;
use std::process::ExitCode;

#[test]
fn correct_accepts_the_global_yes_alias_and_supersedes_the_item() {
    let s = state();
    seed_item(&s, "demo", "itm-c6", "old gist");
    let (code, out, err) = run_write_state(
        "correct",
        &[
            "itm-c6", "new gist", "--reason", "typo fix", "--yes", "--bank", "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(
        payload["correction"]["prior_item_id"],
        json!("itm-c6"),
        "{payload}"
    );
    let new_id = payload["correction"]["new_item_id"]
        .as_str()
        .expect("new revision id");
    let read = call(&s, "get", json!({"bank": "demo", "item_id": new_id}));
    assert_eq!(read["gist"], json!("new gist"), "{read}");
    let prior = call(&s, "get", json!({"bank": "demo", "item_id": "itm-c6"}));
    assert!(prior["valid_until"].is_string(), "{prior}");
}
