// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `compose_context` payload contract over
//! synthetic fixtures: the memory section carries one hit form per included
//! id, the pack's form tally is the fold of those forms, and a gistless
//! snapshot hit renders as a marked reference rather than as a bare id. The
//! same request twice is byte-identical.
//!
//! ## Owns
//! - `compose_payload_carries_forms_and_tally`
//! - `same_request_twice_is_byte_identical`
//! - `gistless_snapshot_hit_is_a_marked_reference`
//! - `retention_profile_writes_stay_in_the_fixture_owned_store`
//! - The retention store of this module's fixture: a unique temporary file
//!   removed when the test ends, never the store the host machine names
//!
//! ## Does not own
//! - Transport framing and parity masking (`mcp_read_transport_conformance`)
//! - The shared read-conformance fixture state (`mcp_read_support`)
//!
//! ## Boundary
//!
//! Changes to the serialized forms, tally, or reference-marker contract should
//! normally require changes here. Changes to ranking, budgeting, or admission
//! should not require changes here.
//!
//! Keep this module focused on the composed pack's form payload.

use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use clio_retrieve::REFERENCE_MARKER;
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Fixture bank. Synthetic ids, gists, and snapshots: no real content.
const BANK: &str = "compose-forms";
/// Fixed fixture timestamp for seeded rows and the persona stamp.
const TS: &str = "2024-01-01T00:00:00Z";
/// The query both seeded lexical docs match.
const QUERY: &str = "alpha planner";

/// A fresh retention-store path under the system temp dir, unique per call.
///
/// The process id separates processes and the atomic counter separates calls
/// inside one, following the workspace's scratch-path pattern: a static call
/// sequence plus the process id under the system temp dir.
fn temp_store_path() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "am-compose-forms-retention-{}-{call}.json",
        std::process::id()
    ))
}

/// The hermetic state plus the retention store file it owns.
///
/// `McpState::open_with_effective` resolves the durable store path from the
/// host environment (`CLIO_RETENTION_PROFILES`, resolved in
/// `McpState::open_with_effective`), so the fixture redirects it before any
/// `retention_profile_set` call, mirroring the provenance tests' own `Fixture`
/// pattern: a profile a test writes never reaches the machine's store, and the
/// file is removed when the fixture ends.
struct Fixture {
    state: Arc<McpState>,
    /// The store file this fixture owns; removed when the fixture ends.
    store_path: PathBuf,
}

impl Fixture {
    /// Open the hermetic state: the effective config is empty, so no embedder
    /// resolves and the dense arm is off. Nothing host-specific can enter a
    /// payload, and the retention store is a file this fixture owns.
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

/// The fixture stands in for the state, so a test reads as one that opens a
/// state directly.
impl std::ops::Deref for Fixture {
    type Target = McpState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// One synthetic episodic item; `None` gist builds the gistless snapshot hit.
fn item(id: &str, gist: Option<&str>) -> MemoryItem {
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
        snapshot: Some(json!({"entity": "planner", "amount": 1, "date": "2024-01-01"})),
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

/// Seed one prose hit (carries a gist) and one gistless snapshot hit, and pin
/// the read to a clock-free path. The engine clock is not boundable through
/// the published `compose_context` tool, so the test disables the retention
/// curve instead of asserting on a host-dependent one; the persona stamp is
/// fixed through the resolved `timestamp` argument. The profile write lands in
/// the fixture-owned store, never the store the host environment names.
fn seed(state: &McpState) {
    let held = dispatch(
        state,
        "retention_profile_set",
        &json!({"bank": BANK, "profile": {"retention_curve": false}, "confirm": true}),
    );
    assert_eq!(held["ok"], json!(true), "{held}");
    put(
        state,
        &item("fx-prose", Some("alpha planner note prose")),
        "alpha planner note prose",
    );
    put(state, &item("fx-snap", None), "alpha planner snapshot bare");
}

/// One fixed-clock `compose_context` call over the seeded bank.
fn compose(state: &McpState) -> Value {
    let pack = dispatch(
        state,
        "compose_context",
        &json!({"bank": BANK, "query": QUERY, "budget_tokens": 400, "timestamp": TS}),
    );
    assert_eq!(pack["ok"], json!(true), "{pack}");
    pack
}

fn section<'a>(pack: &'a Value, name: &str) -> &'a Value {
    pack["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|s| s["name"] == json!(name))
        .unwrap_or_else(|| panic!("{name} section present: {pack}"))
}

/// The memory section carries one form per included id, parallel
/// to `item_ids`, and the pack tally is exactly the fold of those forms.
#[test]
fn compose_payload_carries_forms_and_tally() {
    let state = Fixture::new();
    seed(&state);
    let pack = compose(&state);
    let memory = section(&pack, "memory");
    let ids: Vec<&str> = memory["item_ids"]
        .as_array()
        .expect("item ids")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(ids.len(), 2, "both seeded hits included: {memory}");
    let forms: Vec<&str> = memory["hit_forms"]
        .as_array()
        .expect("hit forms")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(forms.len(), ids.len(), "one form per included id: {memory}");
    for (id, form) in ids.iter().zip(forms.iter()) {
        assert!(
            matches!(*form, "prose" | "exact" | "reference"),
            "`{id}` form `{form}` is not a serialized hit form: {memory}"
        );
    }
    let form_of = |want: &str| forms[ids.iter().position(|id| *id == want).expect("id present")];
    assert_eq!(form_of("fx-prose"), "prose", "gist hit is prose: {memory}");
    assert_eq!(
        form_of("fx-snap"),
        "reference",
        "gistless hit is a reference: {memory}"
    );
    assert_eq!(
        pack["form_tally"],
        json!({"prose": 1, "exact": 0, "reference": 1}),
        "tally is the fold of the section's forms: {pack}"
    );
}

/// The same request twice is byte-identical.
#[test]
fn same_request_twice_is_byte_identical() {
    let state = Fixture::new();
    seed(&state);
    let first = serde_json::to_string(&compose(&state)).expect("serialize first");
    let second = serde_json::to_string(&compose(&state)).expect("serialize second");
    assert_eq!(
        first, second,
        "same request must yield identical pack bytes"
    );
}

/// A gistless snapshot hit renders as a marked reference, never
/// as a bare id, while the gist hit still renders as its prose.
#[test]
fn gistless_snapshot_hit_is_a_marked_reference() {
    let state = Fixture::new();
    seed(&state);
    let pack = compose(&state);
    let memory = section(&pack, "memory");
    let text = memory["text"].as_str().expect("section text");
    let lines: Vec<&str> = text.lines().collect();
    let marked = format!("{REFERENCE_MARKER}fx-snap");
    assert!(
        lines.contains(&marked.as_str()),
        "snapshot reference is the marker plus the snapshot ref: {lines:?}"
    );
    assert!(
        lines.contains(&"alpha planner note prose"),
        "the gist hit contributes its gist as prose: {lines:?}"
    );
    assert!(
        !lines.contains(&"fx-snap"),
        "a bare id is never content: {lines:?}"
    );
    let references: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with(REFERENCE_MARKER))
        .collect();
    assert_eq!(
        references,
        &[marked.as_str()],
        "exactly one marked line: {lines:?}"
    );
}

/// The profile a test writes lands in the fixture's own store file, never in
/// the store the host environment names. Without the fixture's redirect the
/// write goes to the environment-named file or stays in memory, so the
/// read-back below fails on that regression.
#[test]
fn retention_profile_writes_stay_in_the_fixture_owned_store() {
    let state = Fixture::new();
    // The reproduction points the environment at a scratch path that does not
    // exist. When it names an existing file, that file is the operator's own
    // store, so the absence claim is made only for a path that was absent
    // before the write; the read-back alone still fails any redirect removal.
    let absent_host_store = std::env::var_os(clio_config::retention_store::RETENTION_PROFILES_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.exists());
    let held = dispatch(
        &state,
        "retention_profile_set",
        &json!({"bank": BANK, "profile": {"retention_curve": false}, "confirm": true}),
    );
    assert_eq!(held["ok"], json!(true), "{held}");
    assert_eq!(
        held["persisted"],
        json!(true),
        "the write is durable in the fixture's store: {held}"
    );
    let raw =
        std::fs::read_to_string(&state.store_path).expect("the fixture-owned store receives it");
    let stored: Value = serde_json::from_str(&raw).expect("the store file is JSON");
    assert_eq!(
        stored["banks"][BANK]["retention_curve"],
        json!(false),
        "the bank override sits in the fixture-owned file: {raw}"
    );
    if let Some(host) = absent_host_store {
        assert!(
            !host.exists(),
            "a profile write appeared at the environment-named store: {}",
            host.display()
        );
    }
}
