// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `exact` channel's payload over synthetic fixtures: an
//! `exact` pack carries each hit's bounded rendering and its tally counts that
//! form, a `flow` pack does not, the bank's `pack_fidelity_default` applies when
//! the caller names no channel, a rendering that cannot fit the budget falls
//! back to the marked reference, and neither channel exceeds the token budget.
//!
//! ## Owns
//! - `an_exact_pack_carries_a_render_and_the_tally_counts_it`
//! - `a_flow_pack_does_not_carry_a_render`
//! - `the_bank_fidelity_default_applies_and_an_explicit_channel_wins`
//! - `an_over_budget_render_falls_back_to_the_marked_reference`
//! - `total_tokens_never_exceed_the_budget_under_either_channel`
//!
//! ## Does not own
//! - Transport framing and parity masking (`mcp_read_transport_conformance`)
//! - The binding of the two controls onto the request (`read_retrieve_exact_tests`)
//! - The cap and the form selector (`clio-retrieve` `render` and `pack_forms`)
//!
//! ## Boundary
//!
//! Changes to what a channel puts in the packed text, or how the tally reads,
//! should normally require changes here. Changes to budgeting or ranking should
//! not require changes here.
//!
//! Keep this module focused on the composed pack under a fidelity channel.

use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use clio_retrieve::{EXACT_RENDER_MAX_BYTES, PACK_OVERHEAD_TOKENS, REFERENCE_MARKER};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Fixture bank. Synthetic ids, gists, and snapshots: no real content.
const BANK: &str = "compose-exact";
/// Fixed fixture timestamp for seeded rows and the persona stamp.
const TS: &str = "2024-01-01T00:00:00Z";
/// The query every seeded lexical doc matches.
const QUERY: &str = "alpha planner";
/// Pack budget for every call here; large enough for the prose hits.
const BUDGET: usize = 400;

/// A retention-store path under the system temp dir, unique per call.
///
/// The process id separates processes and the atomic counter separates calls
/// inside one, following the workspace's scratch-path pattern.
fn temp_store_path() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "am-compose-exact-retention-{}-{call}.json",
        std::process::id()
    ))
}

/// The hermetic state plus the retention store file it owns.
///
/// The retention store path is redirected before any profile write, so a
/// profile a test writes never reaches the store the host environment names, and
/// the file is removed when the fixture ends.
struct Fixture {
    state: Arc<McpState>,
    /// The store file this fixture owns; removed when the fixture ends.
    store_path: PathBuf,
}

impl Fixture {
    /// Open the hermetic state: the effective config is empty, so no embedder
    /// resolves and the dense arm is off.
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

    /// Switch the retention curve off in the fixture-owned profile, so the
    /// packed page never depends on the host clock.
    fn curve_off(&self) {
        let held = dispatch(
            &self.state,
            "retention_profile_set",
            &json!({"bank": BANK, "profile": {"retention_curve": false}, "confirm": true}),
        );
        assert_eq!(held["ok"], json!(true), "{held}");
    }

    /// Set the bank's stored pack channel in the fixture-owned profile.
    fn bank_channel(&self, channel: &str) {
        let held = dispatch(
            &self.state,
            "retention_profile_set",
            &json!({
                "bank": BANK,
                "profile": {"retention_curve": false, "pack_fidelity_default": channel},
                "confirm": true
            }),
        );
        assert_eq!(held["ok"], json!(true), "{held}");
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

/// One synthetic hit: a prose gist, a snapshot, and a lexical doc that the
/// seeded query matches. A `None` gist builds the gistless snapshot hit.
fn item(id: &str, gist: Option<&str>, snapshot: Value) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.6),
        source_ref: Some("turn-1".into()),
        snapshot: Some(snapshot),
        gist: gist.map(str::to_owned),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        created_at: TS.into(),
        updated_at: TS.into(),
        context: None,
    }
}

/// Persist one item and register its lexical doc so the lexical arm returns it.
fn put(state: &McpState, entry: &MemoryItem, text: &str) {
    state
        .store
        .create_memory_item(entry, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(&entry.bank_id, &entry.id, text)
        .expect("lexical doc");
}

/// A small synthetic snapshot whose rendering fits the cap whole.
fn small_snapshot() -> Value {
    json!({"entity": "planner", "amount": 1})
}

/// A synthetic snapshot whose rendering exceeds the published cap, so it has no
/// rendering at all rather than a shortened one.
fn oversized_snapshot() -> Value {
    json!({"amount": "x".repeat(EXACT_RENDER_MAX_BYTES + 1)})
}

/// Seed one prose hit whose snapshot renders whole.
fn seed_prose(state: &McpState) {
    put(
        state,
        &item(
            "cx-prose",
            Some("alpha planner note prose"),
            small_snapshot(),
        ),
        "alpha planner note prose",
    );
}

/// One `compose_context` call under the given channel; no channel means the
/// bank default decides.
fn compose(state: &McpState, channel: Option<&str>, budget: usize) -> Value {
    let mut args = json!({"bank": BANK, "query": QUERY, "budget_tokens": budget, "timestamp": TS});
    if let Some(channel) = channel {
        args["fidelity"] = json!(channel);
    }
    let pack = dispatch(state, "compose_context", &args);
    assert_eq!(pack["ok"], json!(true), "{pack}");
    pack
}

/// The memory section's packed text, split into lines.
fn memory_lines(pack: &Value) -> Vec<String> {
    pack["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|s| s["name"] == json!("memory"))
        .unwrap_or_else(|| panic!("memory section present: {pack}"))["text"]
        .as_str()
        .expect("section text")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The `exact` channel reads each hit's rendering first, so a rendering that
/// fits contributes it and the pack's tally counts that form.
#[test]
fn an_exact_pack_carries_a_render_and_the_tally_counts_it() {
    let state = Fixture::new();
    state.curve_off();
    seed_prose(&state);
    let pack = compose(&state, Some("exact"), BUDGET);
    assert_eq!(
        pack["form_tally"]["exact"],
        json!(1),
        "the rendering is the packed form: {pack}"
    );
    assert_eq!(
        pack["form_tally"]["prose"],
        json!(0),
        "an exact hit does not also contribute its gist: {pack}"
    );
    let lines = memory_lines(&pack);
    assert!(
        lines.contains(&"{\"amount\":1,\"entity\":\"planner\"}".to_owned()),
        "the packed text carries the snapshot rendered whole: {lines:?}"
    );
    assert!(
        !lines.contains(&"alpha planner note prose".to_owned()),
        "an exact hit does not pack the narrative half: {lines:?}"
    );
}

/// The default channel is the narrative one, so it reads the gist first and a
/// rendering is not the form the hit takes.
#[test]
fn a_flow_pack_does_not_carry_a_render() {
    let state = Fixture::new();
    state.curve_off();
    seed_prose(&state);
    let pack = compose(&state, Some("flow"), BUDGET);
    assert_eq!(
        pack["form_tally"]["prose"],
        json!(1),
        "flow packs the gist: {pack}"
    );
    assert_eq!(
        pack["form_tally"]["exact"],
        json!(0),
        "flow does not pack the rendering: {pack}"
    );
    let lines = memory_lines(&pack);
    assert!(
        lines.contains(&"alpha planner note prose".to_owned()),
        "the packed text carries the gist: {lines:?}"
    );
    assert!(
        !lines.contains(&"{\"amount\":1,\"entity\":\"planner\"}".to_owned()),
        "flow does not pack the rendering: {lines:?}"
    );
}

/// A caller that names no channel takes the bank's stored default, and naming a
/// channel explicitly wins over it. The bank is the fixture's own store, so the
/// default read here is the fixture's and no other bank's.
#[test]
fn the_bank_fidelity_default_applies_and_an_explicit_channel_wins() {
    let state = Fixture::new();
    state.curve_off();
    state.bank_channel("exact");
    seed_prose(&state);
    assert_eq!(
        compose(&state, None, BUDGET)["form_tally"]["exact"],
        json!(1),
        "the bank default applies when the caller names no channel: {:?}",
        compose(&state, None, BUDGET)
    );
    let overridden = compose(&state, Some("flow"), BUDGET);
    assert_eq!(
        overridden["form_tally"]["prose"],
        json!(1),
        "an explicit channel wins over the bank default: {overridden}"
    );
    assert_eq!(
        overridden["form_tally"]["exact"],
        json!(0),
        "the bank default does not also apply: {overridden}"
    );
}

/// A snapshot too large to render has no rendering to pack, so the hit falls to
/// the marked reference rather than being replaced by a bare identifier. The
/// pack is requested under `exact` and the rendered form is never available.
#[test]
fn an_over_budget_render_falls_back_to_the_marked_reference() {
    let state = Fixture::new();
    state.curve_off();
    let snapshot = oversized_snapshot();
    let serialized = serde_json::to_string(&snapshot).expect("serializable fixture");
    assert!(
        serialized.len() > EXACT_RENDER_MAX_BYTES,
        "the fixture snapshot is over the cap, or the case is not exercised: {} bytes",
        serialized.len()
    );
    put(
        &state,
        &item("cx-big", None, snapshot),
        "alpha planner oversized snapshot",
    );
    let pack = compose(&state, Some("exact"), BUDGET);
    assert_eq!(
        pack["form_tally"]["reference"],
        json!(1),
        "an unrenderable hit is a reference: {pack}"
    );
    assert_eq!(
        pack["form_tally"]["exact"],
        json!(0),
        "no rendering is published for it at all: {pack}"
    );
    let lines = memory_lines(&pack);
    assert!(
        lines.contains(&format!("{REFERENCE_MARKER}cx-big")),
        "the packed text marks the reference: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line == "cx-big"),
        "a bare id is never content: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains(serialized.as_str())),
        "an over-cap snapshot is never packed, whole or shortened: {lines:?}"
    );
}

/// The budget is the pack's hard limit, and the rendering is charged against it
/// like any other text, so the reported total never exceeds the budget under
/// either channel. A budget barely above the pack overhead is also exercised,
/// because a pack with almost no room is where an uncharged rendering shows up.
#[test]
fn total_tokens_never_exceed_the_budget_under_either_channel() {
    for budget in [PACK_OVERHEAD_TOKENS + 1, 40, BUDGET] {
        let state = Fixture::new();
        state.curve_off();
        seed_prose(&state);
        for channel in ["flow", "exact"] {
            let pack = compose(&state, Some(channel), budget);
            let total = pack["total_tokens"].as_u64().expect("total tokens");
            assert!(
                total <= budget as u64,
                "a {channel} pack at budget {budget} reports {total} tokens: {pack}"
            );
        }
    }
}
