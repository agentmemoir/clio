// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module pins what one hygiene operation leaves in the durable audit log
//! depending on whether a credential presented it: a bound call names the
//! credential, a local call names nobody.
//!
//! ## Owns
//! - The bound-versus-local transcript through the real tool dispatch path,
//!   with the log rows read back
//! - The rule that a local row carries no credential key at all
//!
//! ## Does not own
//! - Tool argument decoding and the dry-run/confirm shapes (`hygiene_tools_tests`)
//! - The log-row writer itself (`clio-hygiene`)
//!
//! ## Boundary
//!
//! Tests here break when the audited attribution changes; they never change it.
//!
//! Keep this module focused on the credential label an audited row carries.

use super::hygiene_tools_tests::{ok_payload, seed_item, state};
use crate::principal::Principal;
use crate::runtime::McpState;
use crate::write_tools::dispatch;
use clio_types::{Actor, PRINCIPAL_DETAIL_KEY};
use serde_json::{Value, json};
use std::sync::Arc;

/// The same state, scoped to one credential that may reach `bank-mcp`.
fn bound(label: &str) -> Arc<McpState> {
    Arc::new(state().with_principal(Principal::new(
        label.to_owned(),
        Actor::Agent,
        vec!["bank-mcp".to_owned()],
    )))
}

/// Run one confirmed clean and read the durable log row back through the tool.
fn clean_and_read_log(st: &McpState, item_id: &str) -> Value {
    seed_item(st, item_id);
    let cleaned = dispatch(
        st,
        "hygiene_clean",
        &json!({ "item_ids": [item_id], "action": "archive", "confirm": true }),
    );
    assert_eq!(cleaned["ok"], Value::Bool(true), "{cleaned}");
    let listed = dispatch(st, "hygiene_log_list", &json!({ "limit": 10 }));
    let rows = ok_payload(&listed)["rows"]
        .as_array()
        .expect("rows")
        .clone();
    rows.into_iter()
        .find(|r| r["item_id"] == item_id)
        .unwrap_or_else(|| panic!("log row for {item_id}"))
}

#[test]
fn a_bound_hygiene_call_reads_back_naming_its_credential() {
    let st = bound("ops-key");
    let row = clean_and_read_log(&st, "bound-item");
    assert_eq!(row["actor"], "agent", "the claimed role is unchanged");
    assert_eq!(row["detail"][PRINCIPAL_DETAIL_KEY], "ops-key");
    assert_eq!(row["detail"]["result"], "applied");
}

#[test]
fn a_local_hygiene_call_reads_back_naming_nobody() {
    let st = state();
    let row = clean_and_read_log(&st, "local-item");
    assert_eq!(row["actor"], "agent");
    assert!(
        row["detail"].get(PRINCIPAL_DETAIL_KEY).is_none(),
        "an unbound call leaves no credential key, not an empty one: {}",
        row["detail"]
    );
}

#[test]
fn the_two_transports_differ_only_in_the_credential_key() {
    let bound_row = clean_and_read_log(&bound("ops-key"), "diff-bound");
    let local_row = clean_and_read_log(&state(), "diff-local");
    assert_eq!(bound_row["action"], local_row["action"]);
    assert_eq!(bound_row["actor"], local_row["actor"]);
    assert_eq!(bound_row["dry_run"], local_row["dry_run"]);
    assert!(bound_row["detail"].get(PRINCIPAL_DETAIL_KEY).is_some());
    assert!(local_row["detail"].get(PRINCIPAL_DETAIL_KEY).is_none());
}

#[test]
fn a_local_call_cannot_fabricate_a_label_through_its_arguments() {
    let st = state();
    seed_item(&st, "spoof-item");
    // A caller-supplied `principal` argument is not a tool parameter, so it is
    // ignored rather than recorded.
    let cleaned = dispatch(
        &st,
        "hygiene_clean",
        &json!({
            "item_ids": ["spoof-item"],
            "action": "archive",
            "confirm": true,
            "principal": "not-mine",
        }),
    );
    assert_eq!(cleaned["ok"], Value::Bool(true), "{cleaned}");
    let listed = dispatch(&st, "hygiene_log_list", &json!({ "limit": 10 }));
    let rows = ok_payload(&listed)["rows"]
        .as_array()
        .expect("rows")
        .clone();
    let row = rows
        .iter()
        .find(|r| r["item_id"] == "spoof-item")
        .expect("log row");
    assert!(
        row["detail"].get(PRINCIPAL_DETAIL_KEY).is_none(),
        "a local caller cannot name a credential: {}",
        row["detail"]
    );
    assert!(
        !row["detail"].to_string().contains("not-mine"),
        "the argument is not recorded at all: {}",
        row["detail"]
    );
}
