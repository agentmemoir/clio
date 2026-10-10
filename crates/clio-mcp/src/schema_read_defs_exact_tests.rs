// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the two published tool rows that carry the bounded
//! rendering: that the `retrieve` row states what its rendering is, that the
//! `compose_context` row states the channel, its narrative-only warning, its
//! shared budget, and every arm of its fallback chain, and that each published
//! clause names the form a real composed hit takes.
//!
//! ## Owns
//! - `the_published_retrieve_row_states_the_rendering_contract`
//! - `the_published_compose_context_row_states_the_channel_and_the_marker`
//! - `every_published_fallback_clause_names_the_form_the_selector_produced`
//!
//! ## Does not own
//! - The `include_exact` and `fidelity` property text as properties
//!   (`schema_read_props_tests`); the contract test reads the `fidelity` clause
//!   for each arm, because both texts publish the same chain
//! - The score-factor and provenance-floor published text
//!   (`schema_read_defs_tests`)
//! - The runtime binding of either control (`read_retrieve_exact_tests`) and the
//!   composed payload under a channel (`mcp_compose_exact_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! The contract test seeds one synthetic hit per arm into a fixture-owned
//! in-memory store, so every read is order-independent and nothing host-specific
//! takes part: no embedder resolves, the retention curve is switched off through
//! the fixture's own profile, and each fixture removes its store file.
//!
//! Keep this module focused on the published rendering and channel rows.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_retrieve::{
    EXACT_RENDER_MAX_BYTES, HitForm, PACK_OVERHEAD_TOKENS, REFERENCE_MARKER, estimate_tokens,
    render_exact,
};
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// The published description of one read tool row.
fn row_description(tool: &str) -> &'static str {
    crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == tool)
        .unwrap_or_else(|| panic!("{tool} tool definition"))
        .description
}

/// The published `retrieve` row is what a caller reads before deciding whether
/// a hit's rendering is usable, so it names the field, says the rendering is
/// content, says it is absent unless requested, and says it never replaces the
/// exact stored read. Each of the four is asserted separately, because a row
/// that dropped only the "never a substitute" sentence would pass every other
/// assertion here.
#[test]
fn the_published_retrieve_row_states_the_rendering_contract() {
    let description = row_description("retrieve");
    for (claim, needle) in [
        ("the row names the field", "`exact_render`"),
        (
            "the row says what it is a rendering of",
            "a bounded rendering of its authoritative half",
        ),
        ("the row says it is content", "it is content"),
        (
            "the row says it is absent unless requested",
            "it is absent unless the request asks for it",
        ),
        (
            "the row says it never replaces the exact read",
            "never substitutes for the `get_snapshot` read of the exact stored value",
        ),
    ] {
        assert!(
            description.contains(needle),
            "the retrieve row states {claim}: {description}"
        );
    }
}

/// The published `compose_context` row is the channel contract, so it states
/// what each form contributes, warns that the narrative channel must not be the
/// only source of an exact claim, charges the rendering against the same budget,
/// and states each arm of the fallback chain on its own: a hit with no rendering
/// falls back to its gist, and only a hit with nothing to fall back on or a
/// rendering the budget could not hold is marked rather than silently
/// identified.
#[test]
fn the_published_compose_context_row_states_the_channel_and_the_marker() {
    let description = row_description("compose_context");
    for (claim, needle) in [
        (
            "the row names the three forms",
            "Each included hit contributes one form",
        ),
        (
            "the row calls flow the narrative channel",
            "flow (default) is the narrative channel",
        ),
        (
            "the row bars flow from being the only source of an exact claim",
            "must not be the only source of a numeric or identity-bearing claim",
        ),
        (
            "the row points at the exact read for such a claim",
            "read `get_snapshot` for the exact stored value",
        ),
        (
            "the row says exact prefers the rendering",
            "exact prefers each hit's bounded rendering",
        ),
        (
            "the row charges the rendering against the same budget",
            "charged against this same budget_tokens",
        ),
        (
            "the row says a hit with no rendering contributes its gist",
            "A hit with no rendering contributes its gist instead",
        ),
        (
            "the row says a hit with neither half is marked",
            "a hit with neither a rendering nor a gist is marked as a reference \
             rather than silently replaced by its identifier",
        ),
        (
            "the row marks an over-budget rendering's hit too",
            "so is a hit whose rendering did not fit the remaining budget",
        ),
        (
            "the row says the tally counts every form",
            "per-form tally counts every form",
        ),
    ] {
        assert!(
            description.contains(needle),
            "the compose_context row states {claim}: {description}"
        );
    }
    assert!(
        !description.contains("A hit that carries no rendering is marked as a reference"),
        "the row no longer promises a marked reference for a hit that merely \
         lacks a rendering: such a hit contributes its gist, so the old \
         sentence would send a caller looking for a marker that is not there"
    );
}

/// One published property of one tool, located by its published key.
fn published_prop(tool: &str, name: &str) -> Value {
    crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == tool)
        .unwrap_or_else(|| panic!("{tool} tool definition"))
        .props
        .into_iter()
        .find(|(key, _)| *key == name)
        .map_or_else(|| panic!("`{name}` published"), |(_, schema)| schema)
}

/// Fixture bank. Synthetic ids and snapshots only.
const ARM_BANK: &str = "fallback-contract";
/// Fixed fixture timestamp for the seeded row and the persona stamp.
const ARM_TS: &str = "2024-01-01T00:00:00Z";
/// The query the seeded lexical doc matches.
const ARM_QUERY: &str = "alpha planner";
/// The one item id every arm's pack carries.
const ARM_ITEM: &str = "bx-arm";
/// The gist every arm that carries one shares.
const ARM_GIST: &str = "alpha planner note prose";

/// A retention store path under the system temp dir, unique per call. The process
/// id separates processes and the atomic counter separates calls inside one, so
/// no two fixtures read or write each other's profile.
fn temp_store_path() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "am-fallback-contract-retention-{}-{call}.json",
        std::process::id()
    ))
}

/// One hermetic state holding the single synthetic hit of one arm, plus the
/// retention store file it owns.
///
/// The effective config is empty, so no embedder resolves and the dense arm is
/// off; the retention curve is switched off through the fixture-owned profile, so
/// the read never reads the wall clock or the store the host environment names.
struct ArmFixture {
    state: Arc<McpState>,
    /// The store file this fixture owns; removed when the fixture ends.
    store_path: PathBuf,
}

impl ArmFixture {
    /// Open the state and seed the one hit the arm describes.
    fn new(gist: Option<&str>, snapshot: Option<Value>) -> Self {
        let mut state = McpState::open_with_effective(
            &McpOpenOptions {
                default_bank: ARM_BANK.into(),
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
        let off = dispatch(
            &state,
            "retention_profile_set",
            &json!({"bank": ARM_BANK, "profile": {"retention_curve": false}, "confirm": true}),
        );
        assert_eq!(off["ok"], json!(true), "{off}");
        let entry = MemoryItem {
            id: ARM_ITEM.into(),
            bank_id: ARM_BANK.into(),
            subject_id: "subj".into(),
            kind: ItemKind::Episodic,
            category: None,
            episodic_type: Some(EpisodicType::Task),
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            admission_score: Some(0.6),
            source_ref: Some("turn-1".into()),
            snapshot,
            gist: gist.map(str::to_owned),
            valid_from: ARM_TS.into(),
            valid_until: None,
            tx_from: ARM_TS.into(),
            tx_until: None,
            created_at: ARM_TS.into(),
            updated_at: ARM_TS.into(),
            context: None,
        };
        state
            .store
            .create_memory_item(&entry, "agent")
            .expect("create item");
        state
            .index
            .put_lexical_doc(&entry.bank_id, &entry.id, ARM_QUERY)
            .expect("lexical doc");
        Self { state, store_path }
    }
}

impl Drop for ArmFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.store_path);
    }
}

/// One arm of the selector's truth table, with the clauses that publish it.
///
/// The selector is private to `clio-retrieve`, so an arm reaches it the way a
/// caller does: through a real `compose_context` pack, whose `hit_forms` are the
/// selector's own return values.
struct Arm {
    /// A name for the arm, so a failure names the arm rather than a form.
    label: &'static str,
    /// The gist the stored hit carries, if any.
    gist: Option<&'static str>,
    /// The snapshot the stored hit carries, if any.
    snapshot: Option<Value>,
    channel: &'static str,
    /// The call's `budget_tokens`, or `None` for the roomy default.
    budget_tokens: Option<usize>,
    form: HitForm,
    /// The clause the `compose_context` row publishes for this arm.
    row_clause: &'static str,
    /// The clause the `fidelity` property publishes for this arm.
    prop_clause: &'static str,
}

/// Every arm a `compose_context` call can reach, with the clauses publishing it.
///
/// The byte-cap arm and the budget arm are separate rows on purpose. One yields
/// no rendering at all, so the hit falls back to its gist; the other drops a
/// rendering the selector did produce, so the hit takes the marked reference.
/// Published text that merged the two would promise one fallback for two
/// different causes, which is the drift this table exists to catch.
fn arms() -> Vec<Arm> {
    let small = json!({ "entity": "planner" });
    let wide = json!({ "k": "a".repeat(240) });
    let over_cap = json!({ "k": "a".repeat(EXACT_RENDER_MAX_BYTES) });
    let rendering = render_exact(&wide).expect("the wide snapshot is within the byte cap");
    // One token short of the wide rendering, so the selector drops it whole and
    // the marked reference still fits what is left of the budget.
    let tight = PACK_OVERHEAD_TOKENS + estimate_tokens(&rendering) - 1;
    vec![
        Arm {
            label: "flow with a gist",
            gist: Some(ARM_GIST),
            snapshot: Some(small.clone()),
            channel: "flow",
            budget_tokens: None,
            form: HitForm::Prose,
            row_clause: "A hit with no rendering contributes its gist instead",
            prop_clause: "reading each hit's gist first",
        },
        Arm {
            label: "flow with a rendering it never asks for",
            gist: None,
            snapshot: Some(small.clone()),
            channel: "flow",
            budget_tokens: None,
            form: HitForm::Reference,
            row_clause: "a hit with neither a rendering nor a gist is marked as a \
                         reference rather than silently replaced by its identifier",
            prop_clause: "a hit with neither a rendering nor a gist is marked as a \
                          reference rather than a bare identifier",
        },
        Arm {
            label: "exact with a rendering the budget holds",
            gist: Some(ARM_GIST),
            snapshot: Some(small),
            channel: "exact",
            budget_tokens: None,
            form: HitForm::Exact,
            row_clause: "exact prefers each hit's bounded rendering, charged against \
                         this same budget_tokens",
            prop_clause: "exact reads the bounded rendering first and the gist second",
        },
        Arm {
            label: "exact with a rendering over the byte cap",
            gist: Some(ARM_GIST),
            snapshot: Some(over_cap),
            channel: "exact",
            budget_tokens: None,
            form: HitForm::Prose,
            row_clause: "A hit with no rendering contributes its gist instead",
            prop_clause: "A hit whose rendering is absent contributes its gist instead",
        },
        Arm {
            label: "exact with neither half",
            gist: None,
            snapshot: None,
            channel: "exact",
            budget_tokens: None,
            form: HitForm::Reference,
            row_clause: "a hit with neither a rendering nor a gist is marked as a \
                         reference rather than silently replaced by its identifier",
            prop_clause: "a hit with neither a rendering nor a gist is marked as a \
                          reference rather than a bare identifier",
        },
        Arm {
            label: "exact with a rendering over the remaining budget",
            gist: Some(ARM_GIST),
            snapshot: Some(wide),
            channel: "exact",
            budget_tokens: Some(tight),
            form: HitForm::Reference,
            row_clause: "so is a hit whose rendering did not fit the remaining budget",
            prop_clause: "so is a hit whose rendering did not fit the remaining budget",
        },
    ]
}

/// One `compose_context` call over the fixture's single hit, under this arm,
/// reduced to the memory section it packed.
fn compose(fixture: &ArmFixture, arm: &Arm) -> Value {
    let label = arm.label;
    let budget = i64::try_from(arm.budget_tokens.unwrap_or(400)).expect("a budget fits an i64");
    let pack = dispatch(
        &fixture.state,
        "compose_context",
        &json!({
            "bank": ARM_BANK, "query": ARM_QUERY, "budget_tokens": budget,
            "timestamp": ARM_TS, "fidelity": arm.channel
        }),
    );
    assert_eq!(pack["ok"], json!(true), "the {label} arm composed: {pack}");
    pack["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|section| section["name"] == json!("memory"))
        .cloned()
        .unwrap_or_else(|| panic!("the {label} arm packed a memory section: {pack}"))
}

/// The published fallback chain and the selector that picks the form are two
/// halves of one contract, so this drives every arm of the truth table through a
/// real `compose_context` pack and holds both published texts to the form the
/// selector actually produced. A clause naming the wrong outcome, or an arm
/// whose form moved, fails here instead of shipping as prose.
///
/// The packed text is checked as well as the form name, because a form alone
/// would pass even if the section carried other text under it.
#[test]
fn every_published_fallback_clause_names_the_form_the_selector_produced() {
    let row = row_description("compose_context");
    let prop = published_prop("compose_context", "fidelity").to_string();
    for arm in arms() {
        let label = arm.label;
        let form = arm.form;
        let fixture = ArmFixture::new(arm.gist, arm.snapshot.clone());
        let memory = compose(&fixture, &arm);
        let forms: Vec<&str> = memory["hit_forms"]
            .as_array()
            .expect("hit forms")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            forms,
            vec![form.as_str()],
            "the {label} arm packed the form its clause names: {memory}"
        );
        let expected = match form {
            HitForm::Prose => arm.gist.expect("a prose arm carries a gist").to_owned(),
            HitForm::Exact => render_exact(&arm.snapshot.expect("an exact arm stores a snapshot"))
                .expect("the stored snapshot renders within the byte cap"),
            HitForm::Reference => format!("{REFERENCE_MARKER}{ARM_ITEM}"),
        };
        assert_eq!(
            memory["text"].as_str().expect("section text"),
            expected,
            "the {label} arm packed the text its form promises: {memory}"
        );
        assert!(
            row.contains(arm.row_clause),
            "the {label} arm's clause is published in the compose_context row: {row}"
        );
        assert!(
            prop.contains(arm.prop_clause),
            "the {label} arm's clause is published in the fidelity property: {prop}"
        );
    }
}
