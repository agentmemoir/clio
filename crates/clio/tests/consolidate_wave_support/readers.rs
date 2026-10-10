// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

//! This module is responsible for the readers the freshness scenarios assert on:
//! the shapes a stored evidence trail and a recall response take, the seeded bank
//! every one of them starts from, and the literal stamp that makes "this source
//! changed after the reconciliation" a fact about the fixture.
//!
//! ## Owns
//! - `distilled_bank`, `evidence_of`, `current_list`, `entry_ids`, `hit_ids`,
//!   `suppressed_raw_ids`, `claim_set`, `summary_count`, and `MOVED_PAST`
//!
//! ## Does not own
//! - The fixture, the `clio` launcher, and the MCP session
//!   (`consolidate_wave_support`)
//! - The stored-column readers (`distilled_records`)
//!
//! ## Boundary
//!
//! Changes to the shape a scenario reads belong here; changes to what a scenario
//! asserts belong in the harness.
//!
//! Test-only. Never compiled into production builds.

use super::Fixture;
use serde_json::Value;

/// A stamp far after any instant a test run can reach, so "this source changed
/// after the unit was reconciled" is a fact about the fixture rather than about
/// the date the suite ran. A literal, never a computed now.
pub const MOVED_PAST: &str = "2999-01-01T00:00:00Z";

/// A bank seeded and distilled once, with one unit's id: the starting point of
/// every freshness scenario.
///
/// Only the unit under test is inspected by id; the wave distils every hub in the
/// bank, and a scenario that counts by reason narrows the bank with
/// [`Fixture::keep_only_unit`] when it needs a count of one.
pub fn distilled_bank() -> (Fixture, String) {
    let f = Fixture::new();
    f.seed_coactivated_bank(crate::consolidate_wave_support::HUB_ITEMS);
    f.json(&["consolidate", "--force", "--output", "json"]);
    let unit = f
        .distilled_ids()
        .into_iter()
        .next()
        .expect("the wave distilled at least one unit");
    (f, unit)
}

/// A unit's stored evidence trail, parsed.
pub fn evidence_of(f: &Fixture, unit: &str) -> Vec<Value> {
    f.evidence_trail(unit)
}

/// The current revision's evidence list out of a stored trail: the last one,
/// because a later revision is appended and never rewrites an earlier one.
pub fn current_list(trail: &[Value]) -> Value {
    trail
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("the trail has no list: {trail:?}"))
}

/// A unit's current evidence list, its source ids, and one claimed neighbour a
/// scenario can act on.
///
/// A scenario that acts on a unit's claim set needs all three, and picking the
/// wrong id is a different path: a withdrawn hub leaves the unit stale by design,
/// while a withdrawn neighbour is one a wave rebuilds from what is left.
pub fn claim_set(f: &Fixture, unit: &str) -> (Value, Vec<String>, String) {
    let list = current_list(&evidence_of(f, unit));
    let ids = entry_ids(&list);
    let neighbour = ids
        .iter()
        .find(|id| *id != &f.hub_of(unit))
        .cloned()
        .expect("a claimed neighbour, since the hub is a different id");
    (list, ids, neighbour)
}

/// The source ids one evidence list names, in stored order.
pub fn entry_ids(list: &Value) -> Vec<String> {
    list["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("a list carries entries: {list}"))
        .iter()
        .map(|entry| {
            entry["source_id"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry names a source: {entry}"))
                .to_owned()
        })
        .collect()
}

/// The item ids a recall response returned, in page order. The response carries
/// them under `hits`, each with its `item_id`.
pub fn hit_ids(recall: &Value) -> Vec<String> {
    recall["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("a recall carries hits: {recall}"))
        .iter()
        .filter_map(|hit| hit["item_id"].as_str().map(str::to_owned))
        .collect()
}

/// The ids a recall response suppressed as raw items, in stored order.
///
/// The counter is only present when the dedup stage actually suppressed
/// something, so an absent counter reads as an empty list rather than a panic:
/// "nothing was suppressed" is the answer either way.
pub fn suppressed_raw_ids(recall: &Value) -> Vec<String> {
    recall["dedup"]["suppressed_raw_ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
