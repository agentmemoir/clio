// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP `retrieve` provenance floor: the
//! optional argument and the bank's default resolve to one effective floor,
//! both read paths resolve the same way, and a value outside the ladder is
//! refused instead of ignored.
//!
//! ## Owns
//! - The refusal of a mistyped or empty rung: the documented error code, the
//!   wire argument the message names, and the absence of a page
//! - The explicit-over-bank-default resolution, the bank default on an absent
//!   argument, and no floor when neither names one
//! - `compose_context` observing the bank default, and publishing no floor
//!   argument of its own
//! - The retention store of every fixture here: a unique temporary directory
//!   removed when the test ends, never the store the host machine uses
//!
//! ## Does not own
//! - The state, dispatcher call, item, and lexical-write fixtures, which are
//!   shared with the `read_scores` tests (`read_scores_test_support`)
//! - The rung predicate and the fetch-time prune (`clio-retrieve` `floors` and
//!   `hybrid_fetch`)
//! - The wire resolution of the same floor on the in-process JSON surface
//!   (`clio-retrieve` `provenance_surface_tests`)
//! - The published schema text (`schema_read_defs_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the MCP binding of the provenance floor.

use super::read_scores_test_support::{base_item, call, put, state};
use crate::runtime::McpState;
use clio_types::{EpistemicKind, MemoryItem, SourceType};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// The bank these tests store a provenance default for.
const BANK: &str = "provenance-mcp";

/// A second bank that no test in this module stores a profile for. The builtin
/// case therefore names a bank this module never writes, so the two claims about
/// where an effective profile comes from cannot contradict each other.
const UNBANKED: &str = "provenance-mcp-unbanked";

/// Terms the query and every seeded document share. The documents differ in
/// length so their raw lexical scores differ too, and the page order therefore
/// comes from the score rather than a tie-break. No embedder is configured, so
/// the dense arm is off and the lexical arm alone produces the candidates.
const QUERY: &str = "alpha memory";

/// One item at the requested rung on the shared `base_item`: a fact carries no
/// source type, and a belief carries the source type that names its rung, which
/// is the pairing item validation enforces. The confidences differ per class so
/// a rewritten value would be visible.
fn rung_item(bank: &str, id: &str, source: Option<SourceType>) -> MemoryItem {
    let mut item = base_item(id, bank, &format!("{id} memory note"));
    item.epistemic_kind = match source {
        None => EpistemicKind::Fact,
        Some(_) => EpistemicKind::Belief,
    };
    item.source_type = source;
    item.confidence = Some(if source.is_some() { 0.42 } else { 0.9 });
    // The shared fixture names the gist in its snapshot, so the id is restored
    // here to keep the stored row the same shape the assertions describe.
    item.snapshot = Some(json!({"entity": id}));
    item
}

/// One item per rung in `bank`, seeded with a lexical document so the lexical arm
/// can return it. The documents differ in length so their raw lexical scores
/// differ too, and the page order therefore comes from the score rather than a
/// tie-break.
fn seed_rungs(state: &McpState, bank: &str) {
    for (id, source, text) in [
        ("FACT", None, "alpha memory"),
        (
            "STATED",
            Some(SourceType::UserStated),
            "alpha memory stated",
        ),
        (
            "THIRD",
            Some(SourceType::ThirdParty),
            "alpha memory third party reported",
        ),
        (
            "INFERRED",
            Some(SourceType::AgentInferred),
            "alpha memory inferred because of padding",
        ),
    ] {
        put(state, &rung_item(bank, id, source), text);
    }
}

/// The four seeded classes, so a page can be compared against the whole set.
const ALL_CLASSES: [&str; 4] = ["FACT", "STATED", "THIRD", "INFERRED"];

/// The page's item ids, name-sorted so the assertion never depends on the
/// ranking order the score happened to produce.
fn hit_ids(payload: &Value) -> Vec<String> {
    let mut ids: Vec<String> = payload["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .filter_map(|hit| hit["item_id"].as_str().map(str::to_owned))
        .collect();
    ids.sort();
    ids
}

fn sorted(classes: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = classes.iter().map(|id| (*id).to_owned()).collect();
    ids.sort();
    ids
}

fn retrieve(state: &McpState, bank: &str) -> Value {
    call(
        state,
        "retrieve",
        json!({"bank": bank, "query": QUERY, "limit": 10}),
    )
}

fn compose(state: &McpState, bank: &str) -> Value {
    call(
        state,
        "compose_context",
        json!({"bank": bank, "query": QUERY, "budget_tokens": 400}),
    )
}

/// Store a bank profile whose only stated provenance value is `rung`. The
/// other fields take the builtin defaults, so the floor is the single
/// difference this bank states.
fn set_bank_default(state: &McpState, bank: &str, rung: &str) -> Value {
    call(
        state,
        "retention_profile_set",
        json!({
            "bank": bank,
            "profile": {"provenance_floor_default": rung},
            "confirm": true
        }),
    )
}

/// The state plus the unique temporary directory that holds its retention store.
///
/// An opened state reads and writes the durable store the environment names, so
/// the fixture redirects that path before any write: a profile stored here never
/// reaches the machine's store, and the directory is removed when the test ends.
struct Fixture {
    state: Arc<McpState>,
    store_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let store_dir =
            std::env::temp_dir().join(format!("clio-provenance-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&store_dir).expect("the temporary store directory is created");
        let mut state = state();
        Arc::get_mut(&mut state)
            .expect("the fixture state is uniquely owned")
            .set_retention_path(store_dir.join("profiles.json"));
        Self { state, store_dir }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.store_dir);
    }
}

/// The fixture stands in for the state, so a test reads the same as one that
/// opens a state directly.
impl std::ops::Deref for Fixture {
    type Target = McpState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

fn memory_section(pack: &Value) -> &Value {
    pack["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|section| section["name"] == json!("memory"))
        .unwrap_or_else(|| panic!("memory section present: {pack}"))
}

/// A value outside the closed ladder is refused rather than ignored, so a
/// mistyped floor cannot return the page the caller believed was narrowed. The
/// response carries the documented code, names the wire argument the caller
/// sent rather than the store field the value is stored in, and no page at all.
#[test]
fn a_mistyped_provenance_floor_is_refused_instead_of_returning_a_page() {
    let state = Fixture::new();
    seed_rungs(&state, BANK);

    for bad in ["trusted", "Stated", "thirdparty"] {
        let out = call(
            &state,
            "retrieve",
            json!({"bank": BANK, "query": QUERY, "limit": 10, "provenance_floor": bad}),
        );
        assert_eq!(out["ok"], json!(false), "`{bad}` must not succeed: {out}");
        assert_eq!(
            out["code"],
            json!("invalid_argument"),
            "`{bad}` is outside the ladder: {out}"
        );
        let message = out["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("provenance_floor must be"),
            "the refusal names the argument the caller sent: {out}"
        );
        assert!(
            !message.contains("provenance_floor_default"),
            "the store field is not an argument of this tool: {out}"
        );
        assert!(
            out.get("hits").is_none(),
            "a refused floor returns no page at all: {out}"
        );
    }

    // A rung on the ladder is accepted, so the refusals are about the value and
    // not a blanket refusal of the argument.
    let ok = call(
        &state,
        "retrieve",
        json!({"bank": BANK, "query": QUERY, "limit": 10, "provenance_floor": "stated"}),
    );
    assert_eq!(ok["ok"], json!(true), "{ok}");
    assert_eq!(hit_ids(&ok), sorted(&["FACT", "STATED"]), "{ok}");
}

/// An empty string is a present value naming no rung, so it is refused. The
/// optional-string helper would have read it as an absent argument and taken
/// the bank default; the closed ladder is the gate instead, so the two read
/// surfaces agree on the same request.
#[test]
fn an_empty_provenance_floor_is_refused_rather_than_read_as_absent() {
    let state = Fixture::new();
    seed_rungs(&state, BANK);
    let held = set_bank_default(&state, BANK, "third_party");
    assert_eq!(held["ok"], json!(true), "{held}");

    let out = call(
        &state,
        "retrieve",
        json!({"bank": BANK, "query": QUERY, "limit": 10, "provenance_floor": ""}),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
    assert!(out.get("hits").is_none(), "{out}");
}

/// An absent argument and an explicit `null` are the same request, and both
/// take the bank's default.
#[test]
fn an_absent_provenance_floor_takes_the_bank_default() {
    let state = Fixture::new();
    seed_rungs(&state, BANK);
    let held = set_bank_default(&state, BANK, "stated");
    assert_eq!(held["ok"], json!(true), "{held}");
    // The write reached a durable store, so it landed in this test's own
    // directory rather than being held in memory with nowhere to go.
    assert_eq!(held["persisted"], json!(true), "{held}");

    let out = retrieve(&state, BANK);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        hit_ids(&out),
        sorted(&["FACT", "STATED"]),
        "the bank default narrows the page: {out}"
    );

    let explicit_null = call(
        &state,
        "retrieve",
        json!({"bank": BANK, "query": QUERY, "limit": 10, "provenance_floor": null}),
    );
    assert_eq!(explicit_null["ok"], json!(true), "{explicit_null}");
    assert_eq!(hit_ids(&explicit_null), hit_ids(&out), "{explicit_null}");
}

/// The bank default supplies a value and changes nothing else: a named floor
/// wins over it, whether that floor is stricter or looser.
#[test]
fn an_explicit_provenance_floor_wins_over_the_bank_default() {
    let state = Fixture::new();
    seed_rungs(&state, BANK);
    let held = set_bank_default(&state, BANK, "third_party");
    assert_eq!(held["ok"], json!(true), "{held}");

    let stricter = call(
        &state,
        "retrieve",
        json!({"bank": BANK, "query": QUERY, "limit": 10, "provenance_floor": "verified"}),
    );
    assert_eq!(stricter["ok"], json!(true), "{stricter}");
    assert_eq!(
        hit_ids(&stricter),
        sorted(&["FACT"]),
        "`verified` is stricter than the bank default and wins: {stricter}"
    );

    let looser = call(
        &state,
        "retrieve",
        json!({"bank": BANK, "query": QUERY, "limit": 10, "provenance_floor": "inferred"}),
    );
    assert_eq!(looser["ok"], json!(true), "{looser}");
    assert_eq!(
        hit_ids(&looser),
        sorted(&ALL_CLASSES),
        "`inferred` is looser than the bank default and wins: {looser}"
    );
}

/// Neither the caller nor the bank names a floor, so the request imposes none:
/// the builtin profile carries no provenance default and the page keeps every
/// rung. The bank here is the one no test in this module stores a profile for,
/// so the builtin source is this module's own claim rather than the absence of a
/// leftover write.
#[test]
fn an_absent_provenance_floor_with_no_bank_default_keeps_every_rung() {
    let state = Fixture::new();
    seed_rungs(&state, UNBANKED);

    let profile = call(&state, "retention_profile_get", json!({"bank": UNBANKED}));
    assert_eq!(profile["source"], json!("builtin"), "{profile}");
    assert_eq!(
        profile["provenance_floor_default"],
        json!(null),
        "{profile}"
    );

    let out = retrieve(&state, UNBANKED);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(
        hit_ids(&out),
        sorted(&ALL_CLASSES),
        "no floor means every rung: {out}"
    );
}

/// `compose_context` resolves the same bank default for its own memory-section
/// retrieval, so the injection path cannot bypass a policy the bank stated.
#[test]
fn compose_context_takes_the_bank_default_for_its_own_retrieval() {
    let state = Fixture::new();
    seed_rungs(&state, BANK);

    let unbanked = compose(&state, BANK);
    assert_eq!(unbanked["ok"], json!(true), "{unbanked}");
    assert_eq!(
        memory_section(&unbanked)["item_ids"],
        json!(ALL_CLASSES),
        "the builtin profile states no floor: {unbanked}"
    );

    let held = set_bank_default(&state, BANK, "verified");
    assert_eq!(held["ok"], json!(true), "{held}");
    let banked = compose(&state, BANK);
    assert_eq!(banked["ok"], json!(true), "{banked}");
    assert_eq!(
        memory_section(&banked)["item_ids"],
        json!(["FACT"]),
        "the memory section observes the bank default: {banked}"
    );
}

/// `compose_context` publishes no provenance floor of its own: the pack takes
/// the bank default, and a caller reading the schema finds no argument to set.
#[test]
fn compose_context_publishes_no_provenance_floor_argument() {
    let tool = crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "compose_context")
        .expect("compose_context tool definition");
    let names: Vec<&str> = tool.props.iter().map(|(key, _)| *key).collect();
    assert!(
        !names.contains(&"provenance_floor"),
        "the pack resolves the bank default and publishes no floor: {names:?}"
    );
    assert!(
        !tool.required.contains(&"provenance_floor"),
        "and it is never required: {names:?}"
    );
}
