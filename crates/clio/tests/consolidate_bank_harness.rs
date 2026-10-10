// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the end-to-end evidence that the
//! `consolidate` selector is named `bank` on both bindings: it targets the
//! named bank without touching the context bank, the omitted selector keeps
//! the context-bank default, an unknown name does not fall back, and the
//! removed `scope` name is rejected rather than silently ignored.
//!
//! ## Owns
//! - The bank-selection, default-bank, unknown-bank, and removed-name
//!   scenarios
//!
//! ## Does not own
//! - The fixture, the `clio` launcher, and the MCP session
//!   (`consolidate_wave_support`)
//! - The forced-wave scenarios (`consolidate_force_harness`)
//!
//! ## Boundary
//!
//! Changes to how a binding selects or validates a bank should normally
//! require changes here. Changes to the hub algorithm should not require
//! changes here.
//!
//! Keep this module focused on the selector surface.

mod common;
#[path = "consolidate_wave_support/mod.rs"]
mod consolidate_wave_support;

use consolidate_wave_support::{BANK, Fixture, HUB_ITEMS, McpSession, distilled_count};
use serde_json::Value;

/// A second bank, so a selector that fell back to the context bank would be
/// visible in the stored artifacts.
const OTHER_BANK: &str = "otherbank";

#[test]
fn cli_bank_selects_the_named_bank_and_leaves_the_context_bank_alone() {
    let f = Fixture::new();
    f.seed_coactivated_bank(HUB_ITEMS);
    f.seed_coactivated_bank_in(OTHER_BANK, HUB_ITEMS);

    // `--bank` in the leading-global position wins over the bank this
    // fixture's launcher pins, which stands in for the connection default.
    let payload = f.json(&[
        "--bank",
        OTHER_BANK,
        "consolidate",
        "--force",
        "--output",
        "json",
    ]);
    assert_eq!(payload["ok"], Value::Bool(true), "{payload}");
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    assert_eq!(
        distilled_count(payload["hubs"].as_str().unwrap_or_default()),
        Some(u32::try_from(HUB_ITEMS).expect("count fits")),
        "{payload}"
    );
    assert_eq!(
        f.distilled_ids_in(OTHER_BANK).len(),
        HUB_ITEMS,
        "the named bank is the one that ran"
    );
    assert!(
        f.distilled_ids_in(BANK).is_empty(),
        "the context bank must be untouched: {:?}",
        f.distilled_ids_in(BANK)
    );
}

#[test]
fn cli_omitting_bank_targets_the_context_bank() {
    let f = Fixture::new();
    f.seed_coactivated_bank(HUB_ITEMS);

    // No selector anywhere: the connection's bank is the target, which is the
    // default-bank behavior the rename preserves.
    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );
    assert_eq!(f.distilled_ids_in(BANK).len(), HUB_ITEMS);
}

#[test]
fn mcp_bank_selects_the_named_bank_and_leaves_the_context_bank_alone() {
    let f = Fixture::new();
    f.seed_coactivated_bank(HUB_ITEMS);
    f.seed_coactivated_bank_in(OTHER_BANK, HUB_ITEMS);

    let mut session = McpSession::open(&f);
    let queued = session.call(
        2,
        "consolidate",
        serde_json::json!({"bank": OTHER_BANK, "force": true}),
    );
    assert_eq!(
        queued["status"],
        Value::String("accepted".into()),
        "{queued}"
    );
    f.await_hub_wave();
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    assert_eq!(
        f.distilled_ids_in(OTHER_BANK).len(),
        HUB_ITEMS,
        "the named bank is the one that ran"
    );
    assert!(
        f.distilled_ids_in(BANK).is_empty(),
        "the context bank must be untouched: {:?}",
        f.distilled_ids_in(BANK)
    );
}

#[test]
fn mcp_omitting_bank_targets_the_context_bank() {
    let f = Fixture::new();
    f.seed_coactivated_bank(HUB_ITEMS);

    let mut session = McpSession::open(&f);
    let queued = session.call(2, "consolidate", serde_json::json!({"force": true}));
    assert_eq!(
        queued["status"],
        Value::String("accepted".into()),
        "{queued}"
    );
    f.await_hub_wave();
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    assert_eq!(
        f.distilled_ids_in(BANK).len(),
        HUB_ITEMS,
        "the omitted selector stays on the context bank"
    );
}

#[test]
fn an_unknown_bank_name_does_not_fall_back_to_the_context_bank() {
    let f = Fixture::new();
    f.seed_coactivated_bank(HUB_ITEMS);

    // An unknown bank is an empty bank, exactly as it was before the rename:
    // the call is accepted and must do its work there, never in the context
    // bank it was not asked for.
    let mut session = McpSession::open(&f);
    let queued = session.call(2, "consolidate", serde_json::json!({"bank": "ghost"}));
    assert_eq!(
        queued["status"],
        Value::String("accepted".into()),
        "{queued}"
    );
    f.await_hub_wave();
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    assert!(f.distilled_ids_in(BANK).is_empty(), "no fallback allowed");
    assert!(
        f.distilled_ids_in("ghost").is_empty(),
        "an empty bank has no units"
    );
}

#[test]
fn cli_rejects_the_removed_scope_flag() {
    let f = Fixture::new();
    let out = f.output(&["consolidate", "--scope", BANK, "--output", "json"]);
    assert!(!out.status.success(), "the old selector must fail: {out:?}");
    assert_eq!(out.status.code(), Some(2), "a usage error exits 2: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--scope"), "{stderr}");
}

#[test]
fn mcp_rejects_the_removed_scope_argument() {
    let f = Fixture::new();
    let mut session = McpSession::open(&f);
    let result = session.call_raw(2, "consolidate", serde_json::json!({"scope": BANK}));
    assert_eq!(result["isError"], Value::Bool(true), "{result}");
    let structured = &result["structuredContent"];
    assert_eq!(structured["ok"], Value::Bool(false), "{result}");
    assert_eq!(
        structured["code"],
        Value::String("invalid_argument".into()),
        "the rejection carries a usage-shaped code: {result}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}

#[test]
fn both_bindings_select_the_same_bank_for_the_same_arguments() {
    // Byte copies of one two-bank store, so a difference between the bindings
    // cannot come from the fixture.
    let prepared = Fixture::new();
    prepared.seed_coactivated_bank(HUB_ITEMS);
    prepared.seed_coactivated_bank_in(OTHER_BANK, HUB_ITEMS);

    let cli = prepared.copy_of();
    let payload = cli.json(&[
        "--bank",
        OTHER_BANK,
        "consolidate",
        "--force",
        "--output",
        "json",
    ]);
    assert_eq!(
        payload["status"],
        Value::String("completed".into()),
        "{payload}"
    );

    let mcp = prepared.copy_of();
    let mut session = McpSession::open(&mcp);
    let queued = session.call(
        2,
        "consolidate",
        serde_json::json!({"bank": OTHER_BANK, "force": true}),
    );
    assert_eq!(
        queued["status"],
        Value::String("accepted".into()),
        "{queued}"
    );
    mcp.await_hub_wave();
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    assert_eq!(
        cli.distilled_ids_in(OTHER_BANK),
        mcp.distilled_ids_in(OTHER_BANK),
        "the same named bank must distil to the same units on both bindings"
    );
    assert!(
        cli.distilled_ids_in(BANK).is_empty() && mcp.distilled_ids_in(BANK).is_empty(),
        "neither binding may touch the context bank"
    );
}
