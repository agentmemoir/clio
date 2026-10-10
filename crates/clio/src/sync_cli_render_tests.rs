// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the text views of the `clio sync`
//! verbs: the dispatch, the cursor helper, and the default shapes an incomplete
//! payload renders as.
//!
//! ## Owns
//! - Every test of `sync_cli_render`'s views
//!
//! ## Does not own
//! - The verbs and their payloads (see `sync_cli_tests`)
//! - JSON output (see `cli_output`'s tests)
//!
//! ## Boundary
//!
//! Changes to what a sync verb prints in text mode should normally update tests
//! here. Changes to what a verb does should not.
//!
//! Keep this module focused on the text views.

use super::*;
use serde_json::json;

#[test]
fn render_dispatches_to_the_view_named_by_the_verb() {
    assert_eq!(render("sync push", &json!({})), render_transfer(&json!({})));
    assert_eq!(render("sync status", &json!({})), render_status(&json!({})));
    assert_eq!(render("sync serve", &json!({})), render_serve(&json!({})));
}

#[test]
fn the_status_view_renders_one_line_per_bank() {
    let payload = json!({
        "device_id": "dev-1",
        "remote": "h:1",
        "auth": "token",
        "encryption": true,
        "banks": [{"bank_id": "demo", "push_cursor": "c1", "pending_push": 2, "conflicts": 1, "dead_letter_open": 3}]
    });

    assert_eq!(
        render_status(&payload),
        "sync status (device dev-1, remote h:1, auth token, encryption true)\n  demo: push=c1 pull=- pending=2 conflicts=1 dead_letters=3"
    );
}

#[test]
fn the_transfer_view_renders_one_line_per_bank() {
    let transfer = render_transfer(
        &json!({"mode": "push", "banks": [{"bank_id": "demo", "push_cursor": "c"}]}),
    );

    assert!(transfer.contains("  demo: push=c pull=-"), "{transfer}");
}

#[test]
fn the_serve_view_names_the_address_the_banks_and_the_ttl() {
    let serve = render_serve(&json!({"addr": "a:1", "banks": ["demo", "other"]}));

    assert!(serve.contains("sync serve listening on a:1"), "{serve}");
    assert!(serve.contains("banks demo,other"), "{serve}");
}

#[test]
fn every_view_defaults_its_missing_fields() {
    assert_eq!(
        render_status(&json!({})),
        "sync status (device ?, remote (none), auth none, encryption false)"
    );
    assert_eq!(
        render_transfer(&json!({})),
        "sync ? (remote (none), token -, encryption false)"
    );
    assert_eq!(
        render_serve(&json!({})),
        "sync serve listening on ? (banks , ttl 0s, dev false); press Ctrl-C to stop early"
    );
}

#[test]
fn a_cursor_reads_a_string_or_a_dash() {
    assert_eq!(cursor(&json!({"push_cursor": "c1"}), "push_cursor"), "c1");
    assert_eq!(cursor(&json!({}), "push_cursor"), "-");
    assert_eq!(cursor(&json!({"push_cursor": 7}), "push_cursor"), "-");
}
