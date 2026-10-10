// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `stats` tool's behaviour when the bank's
//! retention profile disables the retention curve: the age bands are still
//! reported, each label states its span and boundary in days, and the band
//! counts sum to `items_active` on the hand-computed fixture.
//!
//! ## Owns
//! - The end-to-end path `retention_profile_set` (`retention_curve: false`)
//!   followed by a `stats` tools/call over the MCP handler
//! - The hand-computed two-item fixture: ages 4 and 273 days against the
//!   default 30-day half-life
//! - The fixture-owned retention store the profile write lands in
//!
//! ## Does not own
//! - The band computation and labels (`clio-compliance` stats)
//! - The retention profile object and its storage (`clio-config`)
//! - The pressure counts the report relays (`stats_pressure_test`)
//!
//! ## Boundary
//!
//! Tests here break when a disabled curve changes or removes the stats age
//! bands, or when the profile write stops reaching the bank the stats call
//! reads; they never change production behavior.
//!
//! Keep this module focused on the stats tool with a disabled curve.

use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Bank the profile write disables the curve for and the stats call reads.
const BANK: &str = "stats-curve-off";
/// Fixed "now" for the stats dispatch, so band labels never depend on the clock.
const NOW: &str = "2026-03-01T00:00:00Z";

/// A fresh retention-store path under the system temp dir, unique per call.
///
/// The process id separates processes and the atomic counter separates calls
/// inside one, following the scratch-path pattern of the compose-forms
/// fixture: a static call sequence plus the process id.
fn temp_store_path() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "am-stats-curve-off-retention-{}-{call}.json",
        std::process::id()
    ))
}

/// The hermetic state plus the retention store file it owns.
///
/// The state opens with an empty effective config, so the half-life is the
/// documented 30-day default and no embedder resolves. The retention store is
/// redirected before any `retention_profile_set` call, so a profile a test
/// writes never reaches the store the host machine names; the file is removed
/// when the fixture ends.
struct Fixture {
    state: Arc<McpState>,
    /// The store file this fixture owns; removed when the fixture ends.
    store_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let mut state = McpState::open_with_effective(
            &McpOpenOptions {
                default_bank: BANK.into(),
                ..McpOpenOptions::default()
            },
            &json!({}),
        )
        .expect("state");
        let store_path = temp_store_path();
        // A stale file from a crashed run must not seed this fixture's store.
        let _ = std::fs::remove_file(&store_path);
        Arc::get_mut(&mut state)
            .expect("the fixture state is uniquely owned")
            .set_retention_path(&store_path);
        Self { state, store_path }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.store_path);
    }
}

impl std::ops::Deref for Fixture {
    type Target = McpState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// One semantic item created at `created`, so its age against `NOW` is
/// hand-computed: 4 days for the young row, 273 days for the old row.
fn item(id: &str, created: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: BANK.to_owned(),
        subject_id: id.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.6),
        source_ref: Some(format!("turn-{id}")),
        snapshot: None,
        gist: Some(format!("curve-off fixture row {id}")),
        valid_from: created.to_owned(),
        valid_until: None,
        tx_from: created.to_owned(),
        tx_until: None,
        created_at: created.to_owned(),
        updated_at: created.to_owned(),
        context: None,
    }
}

/// Disable the curve through the real profile tool, seed the hand-computed
/// rows, and prove the write landed in this bank: the stored flag reads
/// false, so the stats call below runs against a truly disabled curve.
fn curve_off_state_with_rows() -> Fixture {
    let fx = Fixture::new();
    let held = dispatch(
        &fx,
        "retention_profile_set",
        &json!({"bank": BANK, "profile": {"retention_curve": false}, "confirm": true}),
    );
    assert_eq!(held["ok"], json!(true), "{held}");
    let raw = std::fs::read_to_string(&fx.store_path).expect("the fixture-owned store file");
    let stored: Value = serde_json::from_str(&raw).expect("the store file is JSON");
    assert_eq!(
        stored["banks"][BANK]["retention_curve"],
        json!(false),
        "the bank's curve is disabled in the fixture-owned file: {raw}"
    );
    for (id, created) in [
        ("itm-new", "2026-02-25T00:00:00Z"), // age 4 days
        ("itm-old", "2025-06-01T00:00:00Z"), // age 273 days
    ] {
        fx.store
            .create_memory_item(&item(id, created), "agent")
            .expect("fixture row");
    }
    fx
}

/// One `stats` tools/call over the MCP handler, returning the report.
fn stats(h: &McpHandler) -> Value {
    let response = h
        .handle_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {"name": "stats", "arguments": {"bank": BANK, "timestamp": NOW}}
        }))
        .expect("response");
    assert!(response.get("error").is_none(), "{response}");
    response["result"]["structuredContent"].clone()
}

/// With the bank's curve disabled, `stats` still reports the four age bands,
/// every label states its span and boundary in days (the unit is stated, not
/// divided by the disabled value), and the band counts sum to `items_active`.
#[test]
fn stats_reports_bands_with_stated_unit_when_the_curve_is_disabled() {
    let fx = curve_off_state_with_rows();
    let handler = McpHandler::new(fx.state.clone());
    let report = stats(&handler);
    let bands = &report["by_age_band"];
    assert_eq!(bands.as_array().map(Vec::len), Some(4), "{report}");
    // "half-li" is the stem of both inflections: "half-life" and "half-lives".
    for band in bands.as_array().expect("four bands") {
        let label = band["label"].as_str().expect("band label");
        assert!(label.contains("half-li"), "{label}");
        assert!(label.contains("days"), "{label}");
    }
    let count = |label: &str| -> u64 {
        match bands
            .as_array()
            .expect("four bands")
            .iter()
            .find(|b| b["label"] == label)
        {
            Some(b) => b["count"].as_u64().expect("count"),
            None => panic!("band `{label}` missing: {report}"),
        }
    };
    assert_eq!(count("0–1 half-life (0–30 days)"), 1, "{report}");
    assert_eq!(count("4+ half-lives (120+ days)"), 1, "{report}");
    let sum: u64 = bands
        .as_array()
        .expect("four bands")
        .iter()
        .map(|b| b["count"].as_u64().expect("count"))
        .sum();
    assert_eq!(
        sum,
        report["items_active"].as_u64().expect("items_active"),
        "{report}"
    );
}
