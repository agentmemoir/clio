// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP read binding of the bounded rendering: that
//! `retrieve` attaches it only when the `include_exact` argument asks for it,
//! and that `compose_context` reads the `fidelity` channel from its own
//! argument and refuses a channel it does not publish.
//!
//! ## Owns
//! - `retrieve_publishes_the_rendering_only_when_asked`
//! - `compose_context_fidelity_binds_the_pack_channel`
//! - `a_bad_fidelity_value_names_the_wire_argument`
//!
//! ## Does not own
//! - The published `compose_context` forms, tally, and marked reference over
//!   the wire (`mcp_compose_exact_test`)
//! - How a bank-defaulted control resolves from its raw argument
//!   (`read_read_policy_tests`)
//! - The rendering cap and the form selector (`clio-retrieve` `render` and
//!   `pack_forms`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the read binding of the rendering controls.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Fixture bank. Synthetic ids and snapshots: no real content.
const BANK: &str = "exact-binding";
/// Fixed fixture timestamp for the seeded row and the persona stamp.
const TS: &str = "2024-01-01T00:00:00Z";
/// The query the seeded lexical doc matches.
const QUERY: &str = "alpha planner";
/// A retention store path unique per call under the system temp dir.
fn temp_store_path() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "am-exact-binding-retention-{}-{call}.json",
        std::process::id()
    ))
}

/// The hermetic state plus the retention store file this test owns.
///
/// The state is opened with an empty effective config, so no embedder resolves
/// and the dense arm is off. The retention store is redirected to a file the
/// test removes when it ends, so a profile write never reaches the store the
/// host environment names. The retention curve is switched off through the
/// fixture-owned profile so the seeded read does not depend on the wall clock.
struct Fixture {
    state: Arc<McpState>,
    /// The store file this fixture owns; removed when the fixture ends.
    store_path: PathBuf,
}

impl Fixture {
    /// Open the state and seed one synthetic hit whose snapshot renders well
    /// under the cap.
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
        let fixture = Self { state, store_path };
        fixture.seed();
        fixture
    }

    /// Disable the retention curve and store one prose hit whose snapshot is
    /// small enough to render whole.
    fn seed(&self) {
        let held = dispatch(
            &self.state,
            "retention_profile_set",
            &json!({"bank": BANK, "profile": {"retention_curve": false}, "confirm": true}),
        );
        assert_eq!(held["ok"], json!(true), "{held}");
        let entry = clio_types::MemoryItem {
            id: "bx-render".into(),
            bank_id: BANK.into(),
            subject_id: "subj".into(),
            kind: clio_types::ItemKind::Episodic,
            category: None,
            episodic_type: Some(clio_types::EpisodicType::Task),
            epistemic_kind: clio_types::EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            admission_score: Some(0.6),
            source_ref: Some("turn-1".into()),
            snapshot: Some(json!({"entity": "planner", "amount": 1})),
            gist: Some("alpha planner note prose".into()),
            valid_from: TS.into(),
            valid_until: None,
            tx_from: TS.into(),
            tx_until: None,
            created_at: TS.into(),
            updated_at: TS.into(),
            context: None,
        };
        self.state
            .store
            .create_memory_item(&entry, "agent")
            .expect("create item");
        self.state
            .index
            .put_lexical_doc(&entry.bank_id, &entry.id, "alpha planner note prose")
            .expect("lexical doc");
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

/// One `retrieve` call over the seeded bank.
#[allow(clippy::needless_pass_by_value)]
fn retrieve(state: &McpState, extra: Value) -> Value {
    let mut args = json!({"bank": BANK, "query": QUERY});
    for (key, value) in extra.as_object().expect("an object of extra args") {
        args[key] = value.clone();
    }
    let out = dispatch(state, "retrieve", &args);
    assert_eq!(out["ok"], json!(true), "{out}");
    out
}

/// The `exact_render` key of the single returned hit.
fn rendered(page: &Value) -> Option<&str> {
    page["hits"]
        .as_array()
        .expect("hits")
        .first()
        .expect("the seeded hit is returned")["exact_render"]
        .as_str()
}

/// The `exact_render` text of every hit, whether it carries the key or not.
fn render_keys(page: &Value) -> Vec<Option<&str>> {
    page["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["exact_render"].as_str())
        .collect()
}

/// The rendering control is additive and opt-in, so a call that omits it leaves
/// every hit exactly as it was: the key is absent, not null and not empty.
#[test]
fn retrieve_publishes_the_rendering_only_when_asked() {
    let state = Fixture::new();
    assert_eq!(
        render_keys(&retrieve(&state, json!({}))),
        vec![None],
        "a call without the toggle carries no rendering key at all: {:?}",
        retrieve(&state, json!({}))
    );
    assert_eq!(
        rendered(&retrieve(&state, json!({"include_exact": true}))),
        Some(r#"{"amount":1,"entity":"planner"}"#),
        "the requested rendering is the snapshot serialized whole"
    );
    assert_eq!(
        rendered(&retrieve(&state, json!({"include_exact": false}))),
        None,
        "an explicit false is the same request as an absent toggle"
    );
}

/// `compose_context` reads the channel from its own argument, so an explicit
/// `exact` packs the rendering even though the bank states no default.
#[test]
fn compose_context_fidelity_binds_the_pack_channel() {
    let state = Fixture::new();
    let packed = dispatch(
        &state,
        "compose_context",
        &json!({
            "bank": BANK, "query": QUERY, "budget_tokens": 400,
            "timestamp": TS, "fidelity": "exact"
        }),
    );
    assert_eq!(packed["ok"], json!(true), "{packed}");
    assert_eq!(
        packed["form_tally"]["exact"],
        json!(1),
        "the explicit channel packs the rendering: {packed}"
    );
}

/// The channel's names are closed, so a mistyped value is refused rather than
/// downgraded to `flow`, and the refusal names the wire argument the caller
/// sent instead of the stored profile field.
#[test]
fn a_bad_fidelity_value_names_the_wire_argument() {
    let state = Fixture::new();
    let refused = dispatch(
        &state,
        "compose_context",
        &json!({
            "bank": BANK, "query": QUERY, "budget_tokens": 400,
            "timestamp": TS, "fidelity": "verbatim"
        }),
    );
    assert_eq!(
        refused["ok"],
        json!(false),
        "an unpublished channel returns no pack: {refused}"
    );
    assert_eq!(
        refused["code"],
        json!("invalid_argument"),
        "an unpublished channel is the documented code: {refused}"
    );
    let message = refused["message"].as_str().expect("error message");
    assert!(
        message.contains("fidelity must be flow|exact"),
        "the refusal names the wire argument: {message}"
    );
    assert!(
        !message.contains("pack_fidelity_default"),
        "the refusal does not name the stored profile field: {message}"
    );
    assert!(
        message.contains("verbatim"),
        "the refusal echoes the value the caller sent: {message}"
    );
}
