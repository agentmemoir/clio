// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the published schema text of the two read controls that
//! carry the bounded rendering: `include_exact` on `retrieve` and `fidelity`
//! on `compose_context`.
//!
//! ## Owns
//! - `the_published_include_exact_property_states_the_rendering_policy`
//! - `the_published_include_exact_property_bounds_tokens_for_any_text`
//! - `the_published_fidelity_property_is_closed_and_has_a_bank_default`
//! - `the_published_compose_context_row_states_the_same_fallback_chain`
//!
//! ## Does not own
//! - The tool row descriptions as rows (`schema_read_defs_exact_tests`)
//! - The other published property blocks (`schema_read_defs_tests`)
//! - The runtime binding of either control (`read_retrieve_exact_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the published rendering-control text.

use clio_retrieve::EXACT_RENDER_MAX_BYTES;
use serde_json::{Value, json};

/// One arm of the published `compose_context` row.
fn row_description(tool: &str) -> String {
    crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == tool)
        .unwrap_or_else(|| panic!("{tool} tool definition"))
        .description
        .to_owned()
}

/// One published property of one read tool, located by its published key.
///
/// The property is cloned so a caller can assert on its JSON without borrowing
/// the tool definition the whole table is walked out of.
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

/// A caller reads this property before asking for a rendering, so it names the
/// field it attaches, states the default, and states the bound with its unit. A
/// bound published without its unit would let a caller read "512" as tokens.
#[test]
fn the_published_include_exact_property_states_the_rendering_policy() {
    let prop = published_prop("retrieve", "include_exact");
    assert_eq!(
        prop["type"],
        json!("boolean"),
        "the request control is a toggle: {prop}"
    );
    let doc = prop.to_string();
    for (claim, needle) in [
        ("the property names the field", "`hits[].exact_render`"),
        (
            "the property states the default",
            "default false, so the key is absent from every hit unless requested",
        ),
        (
            "the property publishes the bound in bytes",
            &format!("{EXACT_RENDER_MAX_BYTES} bytes"),
        ),
        (
            "the property says the bound is never met by shortening",
            "never truncated",
        ),
        (
            "the property says an oversized snapshot yields nothing",
            "yields no rendering for that hit at all",
        ),
        (
            "the property points at the exact read",
            "read `get_snapshot` for the exact value",
        ),
        (
            "the property keeps the rendering out of the trace and warnings",
            "never reaches `warnings`, the `explain` trace, telemetry, or the health surfaces",
        ),
        (
            "the property calls the rendering content",
            "which makes it content",
        ),
    ] {
        assert!(doc.contains(needle), "the property states {claim}: {doc}");
    }
}

/// The cap bounds bytes, and the budget estimates tokens, so the property states
/// the token cost as an upper bound rather than as an equality, and names what
/// reaches that figure: the heuristic counts characters while the cap counts
/// bytes, so a rendering within three characters of the byte count reaches the
/// bound whatever its alphabet. A property that said only ASCII reached it
/// would understate the cost of every multi-byte rendering a caller sends.
#[test]
fn the_published_include_exact_property_bounds_tokens_for_any_text() {
    let doc = published_prop("retrieve", "include_exact").to_string();
    for (claim, needle) in [
        (
            "the property says the cost is an upper bound, not an equality",
            "upper bound rather than an equality",
        ),
        (
            "the property gives the figure",
            &format!("at most {} tokens", EXACT_RENDER_MAX_BYTES / 4),
        ),
        (
            "the property says the heuristic counts characters, not bytes",
            "which counts characters rather than bytes",
        ),
        (
            "the property says multi-byte text reaches the figure too",
            "within three characters of that byte count reaches that figure even \
             when the text holds multi-byte characters",
        ),
        (
            "the property says only a smaller character count lands below it",
            "only a rendering of fewer characters lands below it",
        ),
    ] {
        assert!(doc.contains(needle), "the property states {claim}: {doc}");
    }
}

/// The channel's names are closed and the bank default fills an omitted one, so
/// a caller can pick a channel from the schema alone and knows a mistyped name
/// is refused rather than downgraded.
///
/// The fallback chain is pinned clause by clause rather than as one sentence,
/// because the three clauses are three separate arms of the selector and a row
/// that merged them would pass a single-substring check while promising a
/// fallback the selector never takes.
#[test]
fn the_published_fidelity_property_is_closed_and_has_a_bank_default() {
    let prop = published_prop("compose_context", "fidelity");
    assert_eq!(prop["type"], json!("string"), "{prop}");
    assert_eq!(
        prop["enum"],
        json!(["flow", "exact"]),
        "the two channels are the whole set: {prop}"
    );
    let doc = prop.to_string();
    for (claim, needle) in [
        (
            "the property states what flow reads first",
            "reading each hit's gist first",
        ),
        (
            "the property states what exact reads first",
            "exact reads the bounded rendering first",
        ),
        ("the mistyped name is refused", "invalid_argument"),
        (
            "a refused name is never packed as flow",
            "rather than packed as flow",
        ),
        (
            "the bank default fills an omitted channel",
            "the bank's own pack_fidelity_default applies",
        ),
        (
            "a bank naming none packs flow",
            "a bank that states none packs flow",
        ),
        (
            "the rendering is charged against the same budget",
            "charged against the same budget_tokens",
        ),
        (
            "the bound is published in bytes",
            &format!("{EXACT_RENDER_MAX_BYTES} bytes"),
        ),
        (
            "the byte bound is separate from the budget",
            "is separately bounded at",
        ),
        (
            "an over-budget rendering is dropped whole",
            "is dropped whole for that hit rather than shortened",
        ),
        (
            "a snapshot over the byte bound contributes no rendering",
            "a snapshot over that byte bound contributes no rendering at all",
        ),
        (
            "a hit with no rendering contributes its gist",
            "A hit whose rendering is absent contributes its gist instead",
        ),
        (
            "the fallback is a marked reference",
            "is marked as a reference rather than a bare identifier",
        ),
        (
            "an over-budget rendering's hit is marked too",
            "so is a hit whose rendering did not fit the remaining budget",
        ),
        (
            "the tally cannot disagree with the text",
            "the packed text and the pack's per-form tally cannot disagree",
        ),
    ] {
        assert!(doc.contains(needle), "the property states {claim}: {doc}");
    }
}

/// The published fallback clauses live in the `compose_context` row as well as
/// in the `fidelity` property, and a caller reads the row first. Each clause is
/// pinned in both places, so a fix that corrected one text and left the other
/// promising the old chain fails here.
#[test]
fn the_published_compose_context_row_states_the_same_fallback_chain() {
    let doc = row_description("compose_context");
    for (claim, needle) in [
        (
            "the row says a hit with no rendering contributes its gist",
            "A hit with no rendering contributes its gist instead",
        ),
        (
            "the row marks a hit with neither rendering nor gist",
            "a hit with neither a rendering nor a gist is marked as a reference \
             rather than silently replaced by its identifier",
        ),
        (
            "the row marks a hit whose rendering did not fit the budget",
            "so is a hit whose rendering did not fit the remaining budget",
        ),
    ] {
        assert!(doc.contains(needle), "the row states {claim}: {doc}");
    }
}
