// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! This module is responsible for the end-to-end evidence that a wave records
//! what supported a consolidated claim and stops recording when nothing changed:
//! the trail a first distillation writes, the proof count that follows it, a
//! no-op wave that appends nothing, a source that moves, a source that is
//! withdrawn, and a reconcile that makes a lapsed unit current again.
//!
//! ## Owns
//! - The wave-side freshness scenarios over the real binary and its stored rows
//!
//! ## Does not own
//! - The evidence shape, the freshness decision, and the report counters
//!   (`clio-store`, `clio-write`), and the freshness rendering (`clio-mcp`)
//! - The read surfaces and the recall split
//!   (`consolidated_freshness_reads`)
//! - The wave's own outcomes and the readers (`consolidate_wave_support`)
//!
//! ## Boundary
//!
//! Changes to what a wave must record as proof of a claim, and to what makes a
//! recorded claim lapse, should normally require changes here. Changes to the
//! surfaces that report either should require the other harness.
//!
//! Test-only. Never compiled into production builds.

mod common;
#[path = "consolidate_wave_support/mod.rs"]
mod consolidate_wave_support;

use consolidate_wave_support::{
    MOVED_PAST, McpSession, claim_set, current_list, distilled_bank, entry_ids, evidence_of,
    summary_count,
};
use serde_json::{Value, json};

/// A fresh distillation records one list for revision 1, one entry per claimed
/// source, and a proof count equal to the claim set. The proof count is the
/// phase's headline: it is what makes "how well supported is this" countable.
#[test]
fn a_fresh_distillation_records_one_entry_per_claimed_source() {
    let (f, unit) = distilled_bank();
    let lists = evidence_of(&f, &unit);
    assert_eq!(lists.len(), 1, "one list per revision: {lists:?}");
    assert_eq!(lists[0]["revision"], json!(1), "{lists:?}");

    // The claim set and the evidence name the same sources, so a count can never
    // disagree with what the unit says it summarizes.
    let claimed: Vec<String> = f
        .distilled_records()
        .into_iter()
        .find(|(id, ..)| id == &unit)
        .map(|(_, _, hub, json)| {
            let mut ids: Vec<String> =
                serde_json::from_str(&json).expect("claim set is a JSON array");
            ids.push(hub);
            ids.sort();
            ids.dedup();
            ids
        })
        .expect("a record for the unit");
    assert_eq!(
        entry_ids(&lists[0]),
        claimed,
        "one entry per claimed source"
    );
    assert_eq!(
        entry_ids(&lists[0]).len(),
        claimed.len(),
        "the proof count is the distinct sources, and each appears once"
    );

    // Every entry is an id and a stamp. Nothing else can be in there, so a scan
    // for a text field has to come back empty.
    for entry in lists[0]["entries"].as_array().expect("entries") {
        let object = entry.as_object().expect("an entry is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["as_of", "source_id"], "{entry}");
    }
    let text = serde_json::to_string(&lists).expect("trail text");
    assert!(
        !text.contains("hub probe note") && !text.contains("related to"),
        "the trail holds membership, not the text a summary is built from: {text}"
    );
}

/// A wave over an unchanged cluster appends nothing and rewrites nothing: the
/// stored trail and the item's stamp are byte-identical afterwards.
#[test]
fn a_wave_over_an_unchanged_cluster_appends_nothing() {
    let (f, unit) = distilled_bank();
    let before = evidence_of(&f, &unit);
    let records_before = f.distilled_records();

    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    let summary = payload["hubs"].as_str().expect("hubs summary");
    assert_eq!(
        summary_count(summary, "revised"),
        Some(0),
        "nothing changed, so nothing was revised: {summary}"
    );
    assert_eq!(evidence_of(&f, &unit), before, "the trail is untouched");
    assert_eq!(
        f.distilled_records(),
        records_before,
        "and so is the rest of the record"
    );
    assert_eq!(evidence_of(&f, &unit).len(), 1, "still exactly one list");
}

/// A source's last change moves past the unit's last reconciliation: the unit
/// reads stale for `source_changed` with no wave at all, and the wave that
/// follows reports it by reason **and writes it onto the record**, so recall —
/// which reads only the item's snapshot — sees the lapse too.
#[test]
fn a_source_that_moved_is_stale_before_any_wave_runs() {
    let (f, unit) = distilled_bank();
    // One unit, so a count of one is this unit's count and not the bank's.
    f.keep_only_unit(&unit);
    let (_, claimed, source) = claim_set(&f, &unit);
    assert!(
        claimed.len() > 1,
        "the claim set names a neighbour: {claimed:?}"
    );
    f.set_item_updated_at(&source, MOVED_PAST);

    // No wave has run since the move, so the read is the whole mechanism.
    let mut session = McpSession::open(&f);
    let got = session.call(2, "get", json!({"item_id": unit}));
    let block = &got["freshness"];
    assert_eq!(block["stale"], json!(true), "{got}");
    assert_eq!(block["stale_reason"], json!("source_changed"), "{got}");
    assert_eq!(
        block["revision"],
        json!(1),
        "still the revision the wave wrote: {got}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    // And the wave's own report counts it by reason, so an operator can see that
    // the bank is not current without reading any item.
    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    let summary = payload["hubs"].as_str().expect("hubs summary");
    assert_eq!(
        summary_count(summary, "stale_source_changed"),
        Some(1),
        "the moved source's unit is counted as changed: {summary}"
    );
    assert_eq!(
        summary_count(summary, "stale_source_removed"),
        Some(0),
        "and nothing is counted as removed: {summary}"
    );

    // The wave also wrote the reason onto the record, so the retrieve path — which
    // reads the item's snapshot and nothing else — can see the lapse.
    assert_eq!(
        f.stale_reason_of(&unit),
        "source_changed",
        "the record carries the reason recall will read"
    );
    let mut session = McpSession::open(&f);
    let inspected = session.call(2, "inspect", json!({"limit": 50}));
    let row = inspected["items"]
        .as_array()
        .expect("inspect rows")
        .iter()
        .find(|r| r["item_id"] == json!(unit.as_str()))
        .unwrap_or_else(|| panic!("the unit is not listed: {inspected}"));
    assert_eq!(
        row["freshness"]["stale_reason"],
        json!("source_changed"),
        "and the read surface still agrees with it: {row}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}

/// Withdrawing a quoted source with the real `discard` verb: the unit reads stale
/// for `source_removed`, its current proof view stops counting the withdrawn
/// source, and the trail keeps the entry as history of what was claimed then.
#[test]
fn a_withdrawn_source_leaves_the_unit_stale_and_out_of_its_proof_count() {
    let (f, unit) = distilled_bank();
    // A real neighbour, not the hub: `claim_set` excludes the unit's hub, so this
    // test exercises the withdrawn-source path it names.
    let (_, _, source) = claim_set(&f, &unit);
    f.assert_ok(&[
        "discard",
        &source,
        "--reason",
        "withdrawn by the operator",
        "--confirm",
        "--output",
        "json",
    ]);

    let after = evidence_of(&f, &unit);
    let current = current_list(&after);
    assert!(
        !entry_ids(&current).contains(&source),
        "the withdrawn source is not counted as current evidence: {after:?}"
    );
    // The record keeps the same single list, because the cascade writes no
    // revision: the entry is dropped from the current revision's list, and there
    // is no earlier revision to keep.
    assert_eq!(after.len(), 1, "still one list: {after:?}");
    let entries = current["entries"].as_array().expect("entries");
    for entry in entries {
        assert!(
            entry["as_of"].is_string(),
            "a surviving entry still carries its own stamp: {entry}"
        );
    }

    let mut session = McpSession::open(&f);
    let got = session.call(2, "get", json!({"item_id": unit}));
    assert_eq!(got["freshness"]["stale"], json!(true), "{got}");
    assert_eq!(
        got["freshness"]["stale_reason"],
        json!("source_removed"),
        "{got}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}

/// The report counts lapsed units by reason, and no lapsed unit is ever counted
/// as current: one bank holding a unit whose source was withdrawn and a unit
/// whose source moved reports one of each, and neither appears in the count of
/// units the wave left alone.
#[test]
fn the_report_counts_lapsed_units_by_reason_and_never_counts_one_as_current() {
    let (f, removed_unit) = distilled_bank();
    // Withdrawing a unit's own HUB is the one withdrawal a wave never repairs: a
    // unit whose hub left active use is reported and left alone, so its stored
    // `source_removed` survives the wave. Withdrawing a neighbour instead is
    // rebuilt from what is left, and would leave nothing stale to count.
    let hub = f.hub_of(&removed_unit);
    f.assert_ok(&[
        "discard",
        &hub,
        "--reason",
        "withdrawn by the operator",
        "--confirm",
        "--output",
        "json",
    ]);

    // A source that unit does not claim, so moving it lapses a different unit and
    // the two reasons cannot collapse into one.
    let claimed: std::collections::BTreeSet<String> =
        entry_ids(&current_list(&evidence_of(&f, &removed_unit)))
            .into_iter()
            .collect();
    let moved = f
        .stored_item_ids()
        .into_iter()
        .find(|id| !claimed.contains(id))
        .expect("the bank holds items this unit does not claim");
    f.set_item_updated_at(&moved, MOVED_PAST);

    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    let summary = payload["hubs"].as_str().expect("hubs summary");
    assert!(
        summary_count(summary, "stale_source_removed").is_some_and(|n| n >= 1),
        "the unit whose hub was withdrawn is counted: {summary}"
    );
    assert!(
        summary_count(summary, "stale_source_changed").is_some_and(|n| n >= 1),
        "the moved source is counted under its own reason: {summary}"
    );
    assert_eq!(
        summary_count(summary, "stale_revision_rejected"),
        Some(0),
        "and nothing was refused: {summary}"
    );
    assert_eq!(
        summary_count(summary, "stale_unknown"),
        Some(0),
        "and nothing was undecidable, because every stamp here is readable: {summary}"
    );
    // The total is the sum of the reasons, so no lapsed unit is missing from the
    // count and none is counted twice. A unit may also appear in the wave's own
    // outcome buckets, because the two answer different questions.
    let reasons: u32 = [
        "stale_source_removed",
        "stale_source_changed",
        "stale_revision_rejected",
        "stale_unknown",
    ]
    .iter()
    .filter_map(|field| summary_count(summary, field))
    .sum();
    assert_eq!(
        summary_count(summary, "stale"),
        Some(reasons),
        "the total is the sum of the reasons: {summary}"
    );
    assert!(
        reasons >= 2,
        "both kinds of lapse are present, so the counts are not one number twice: {summary}"
    );
}

/// A reconcile after a lapse makes the unit current again, and clears the stale
/// state and moves the reconciliation time in the same event.
#[test]
fn a_reconcile_after_a_lapse_clears_it_and_moves_the_reconciliation_time() {
    let (f, unit) = distilled_bank();
    // The unit whose claim set this scenario acts on is the one that quotes the
    // source it withdraws. In a fully connected bank every unit quotes every
    // other, so the wave marks several units at once; this scenario is about one
    // unit's own two revisions, and the assertions below are on that unit's
    // stored state rather than on the bank's counts.
    let (first, first_ids, withdrawn) = claim_set(&f, &unit);
    let first_reconciled_at = first["reconciled_at"]
        .as_str()
        .expect("a reconciliation time")
        .to_owned();

    f.assert_ok(&[
        "discard",
        &withdrawn,
        "--reason",
        "withdrawn by the operator",
        "--confirm",
        "--output",
        "json",
    ]);

    // The cascade writes no revision, so at this point revision 1 IS the current
    // one and the withdrawn source leaves its list. Captured here so the wave's
    // own append can be shown not to have rewritten it.
    let marked = current_list(&evidence_of(&f, &unit));
    let marked_ids = entry_ids(&marked);
    assert!(
        !marked_ids.contains(&withdrawn),
        "the withdrawn source left the current list: {marked}"
    );
    assert_eq!(
        marked_ids.len(),
        first_ids.len() - 1,
        "and nothing else did: {marked}"
    );

    let mut session = McpSession::open(&f);
    let lapsed = session.call(2, "get", json!({"item_id": unit}));
    assert_eq!(lapsed["freshness"]["stale"], json!(true), "{lapsed}");
    assert_eq!(
        lapsed["freshness"]["stale_reason"],
        json!("source_removed"),
        "{lapsed}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );

    let payload = f.json(&["consolidate", "--force", "--output", "json"]);
    let summary = payload["hubs"].as_str().expect("hubs summary");
    // The seeded bank is fully connected, so every unit that quoted the
    // withdrawn neighbour is re-derived, not only this one. The per-unit
    // assertions below are what pin this unit's own two revisions; the summary
    // only has to show the wave did the work.
    assert!(
        summary_count(summary, "revised").is_some_and(|n| n >= 1),
        "the wave re-derived at least the unit under test: {summary}"
    );
    assert_eq!(
        summary_count(summary, "stale_revision_rejected"),
        Some(0),
        "and nothing was refused: {summary}"
    );

    // The record is keyed by the unit id while the current revision's item is
    // `{unit}:v2`, so the block only resolves because the snapshot names the
    // record it belongs to.
    let rev2 = format!("{unit}:v2");
    let mut session = McpSession::open(&f);
    let after = session.call(2, "get", json!({"item_id": rev2}));
    assert_eq!(after["id"], json!(rev2.as_str()), "{after}");
    assert_eq!(
        after["snapshot"]["unit_id"],
        json!(unit.as_str()),
        "{after}"
    );
    let block = &after["freshness"];
    assert_eq!(block["stale"], json!(false), "{after}");
    assert_eq!(block["stale_reason"], Value::Null, "and no reason: {after}");
    assert_eq!(block["revision"], json!(2), "{after}");
    assert_ne!(
        block["last_reconciled_at"].as_str(),
        Some(first_reconciled_at.as_str()),
        "the reconciliation time moved with the stale state, in the same event: {after}"
    );

    let trail = block["evidence"].as_array().expect("trail");
    assert_eq!(
        trail.len(),
        2,
        "the reconcile appended a list and kept the prior one: {after}"
    );
    assert_eq!(trail[0]["revision"], json!(1), "{after}");
    assert_eq!(trail[1]["revision"], json!(2), "{after}");
    assert_eq!(
        trail[0]["reconciled_at"].as_str(),
        Some(first_reconciled_at.as_str()),
        "and the prior list still carries the prior stamp: {after}"
    );
    assert_eq!(
        entry_ids(&trail[0]),
        marked_ids,
        "the wave appended; it did not rewrite the list the cascade left: {after}"
    );
    let current_ids = entry_ids(&trail[1]);
    assert!(
        !current_ids.contains(&withdrawn),
        "the current list no longer claims the withdrawn source: {after}"
    );
    assert_eq!(
        block["proof_count"].as_u64(),
        Some(current_ids.len() as u64),
        "and the count follows the current list: {after}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}
