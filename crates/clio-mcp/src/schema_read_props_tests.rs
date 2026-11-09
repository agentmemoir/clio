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
//! - `the_published_include_exact_property_never_truncates_an_oversized_snapshot`
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
/// field it attaches, states the default, and publishes the byte bound with its
/// unit. A bound published without its unit would let a caller read "512" as
/// tokens.
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
        ("the property names the field", "hits[].exact_render"),
        ("the property states the default", "default false"),
        (
            "the property publishes the bound in bytes",
            &format!("{EXACT_RENDER_MAX_BYTES} bytes"),
        ),
        ("the property points at the exact read", "get_snapshot"),
    ] {
        assert!(doc.contains(needle), "the property states {claim}: {doc}");
    }
}

/// The cap bounds bytes, so an oversized snapshot is dropped whole rather than
/// shortened. A caller who reads a shortened value as the stored one would take
/// a prefix for the record, so the published text says the bound never
/// truncates.
#[test]
fn the_published_include_exact_property_never_truncates_an_oversized_snapshot() {
    let doc = published_prop("retrieve", "include_exact").to_string();
    for (claim, needle) in [
        (
            "the property says the rendering is bounded",
            "a bounded snapshot",
        ),
        (
            "the property says the bound never truncates",
            "never truncated",
        ),
        ("the property points at the exact read", "see get_snapshot"),
    ] {
        assert!(doc.contains(needle), "the property states {claim}: {doc}");
    }
}

/// The channel's names are closed and the bank default fills an omitted one, so
/// a caller can pick a channel from the schema alone and knows which half each
/// one reads first.
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
            "flow (default) reads gist before rendering",
        ),
        (
            "the property states what exact reads first",
            "exact the reverse",
        ),
        (
            "the bank default fills an omitted channel",
            "Omitted uses the bank default",
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
