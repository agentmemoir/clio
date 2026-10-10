// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the store-backed gated-write signal source.
//!
//! ## Owns
//! - Store-backed neighbor signal tests and write-time-independence tests
//! - Damaged-row resilience tests for admission lookups
//!
//! ## Does not own
//! - Production signal-source behavior
//! - Admission scoring or durable item storage
//!
//! ## Boundary
//!
//! Changes to store-backed admission signals should normally require changes here.
//! Changes to admission scoring should not require changes here.
//!
//! Keep this module focused on store-backed admission signal tests.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use clio_admission::gated::candidate_text;
use clio_belief::signals::BeliefSignalSource;
use clio_store::{DekProvider, LocalDevKms, SqliteStore};
use serde_json::json;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

fn signals_for(s: &McpState) -> StoreSignals {
    StoreSignals::new(Arc::clone(&s.store), Arc::clone(&s.index))
}

fn item(bank: &str, id: &str, gist: &str, created_at: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: created_at.into(),
        valid_until: None,
        tx_from: created_at.into(),
        tx_until: None,
        created_at: created_at.into(),
        updated_at: created_at.into(),
        context: None,
    }
}

#[test]
fn empty_bank_has_no_neighbors() {
    let s = state();
    let signals = signals_for(&s);
    let probe = item("bank-empty", "probe", "hello world", "2026-01-02T00:00:00Z");
    let neighbors = SignalSource::neighbor_texts(&signals, &probe).expect("neighbors");
    assert!(neighbors.is_empty(), "{neighbors:?}");
}

#[test]
fn duplicate_text_is_returned_as_neighbor() {
    let s = state();
    let prior = item(
        "bank-dup",
        "itm-prior",
        "Satoshi lives in Japan",
        "2026-01-01T00:00:00Z",
    );
    s.store.create_memory_item(&prior, "agent").expect("seed");
    let signals = signals_for(&s);
    let probe = item(
        "bank-dup",
        "probe",
        "Satoshi lives in Japan",
        "2026-01-02T00:00:00Z",
    );
    let neighbors = SignalSource::neighbor_texts(&signals, &probe).expect("neighbors");
    assert!(neighbors.contains(&candidate_text(&prior)), "{neighbors:?}");
}

#[test]
fn self_id_is_excluded() {
    let s = state();
    let only = item(
        "bank-self",
        "itm-only",
        "unique text here",
        "2026-01-01T00:00:00Z",
    );
    s.store.create_memory_item(&only, "agent").expect("seed");
    let signals = signals_for(&s);
    let neighbors = SignalSource::neighbor_texts(&signals, &only).expect("neighbors");
    assert!(neighbors.is_empty(), "{neighbors:?}");
}

#[test]
fn unparseable_timestamps_do_not_break_the_neighbor_scan() {
    let s = state();
    let prior = item("bank-bad-ts", "itm-prior", "some text", "not-a-timestamp");
    s.store.create_memory_item(&prior, "agent").expect("seed");
    let signals = signals_for(&s);
    let probe = item("bank-bad-ts", "probe", "other text", "also-bad");
    assert_eq!(
        SignalSource::neighbor_texts(&signals, &probe)
            .expect("neighbors")
            .len(),
        1
    );
}

#[test]
fn unreadable_neighbor_is_skipped_while_readable_neighbor_remains() {
    let kms = Arc::new(LocalDevKms::new());
    let store = Arc::new(
        SqliteStore::open_in_memory(Arc::clone(&kms) as Arc<dyn DekProvider>).expect("store"),
    );
    let mut readable = item(
        "bank-damaged",
        "itm-readable",
        "readable neighbor",
        "2026-01-01T00:00:00Z",
    );
    readable.subject_id = "readable-subject".into();
    let mut unreadable = item(
        "bank-damaged",
        "itm-unreadable",
        "unreadable neighbor",
        "2026-01-02T00:00:00Z",
    );
    unreadable.subject_id = "unreadable-subject".into();
    store
        .create_memory_item(&readable, "agent")
        .expect("seed readable");
    store
        .create_memory_item(&unreadable, "agent")
        .expect("seed unreadable");
    kms.destroy_dek("unreadable-subject")
        .expect("shred unreadable key");

    let signals = StoreSignals::new(store.clone(), store);
    let probe = item(
        "bank-damaged",
        "probe",
        "new candidate",
        "2026-01-03T00:00:00Z",
    );
    let neighbors = SignalSource::neighbor_texts(&signals, &probe).expect("signals");
    assert_eq!(neighbors, vec![candidate_text(&readable)]);
}

#[test]
fn empty_bank_id_fails_closed() {
    let s = state();
    let signals = signals_for(&s);
    let probe = item("", "probe", "text", "2026-01-02T00:00:00Z");
    assert!(SignalSource::neighbor_texts(&signals, &probe).is_err());
}

/// Work is bounded by the configured recent window: a probe with no lexical
/// overlap never sees more than `recent_window` neighbors, regardless of how
/// many items the bank holds.
#[test]
fn neighbor_work_is_bounded_by_the_recent_window() {
    let s = state();
    for i in 0..20 {
        let seeded = item(
            "bank-bound",
            &format!("itm-{i:02}"),
            &format!("stored fact number {i}"),
            &format!("2026-01-{:02}T00:00:00Z", i + 1),
        );
        s.store.create_memory_item(&seeded, "agent").expect("seed");
    }
    let signals = StoreSignals::with_limits(Arc::clone(&s.store), Arc::clone(&s.index), 3, 0);
    let probe = item(
        "bank-bound",
        "probe",
        "zzz nonexistent qqq",
        "2026-02-01T00:00:00Z",
    );
    let neighbors = SignalSource::neighbor_texts(&signals, &probe).expect("neighbors");
    assert!(
        neighbors.len() <= 3,
        "bounded to 3, got {}",
        neighbors.len()
    );
    assert_eq!(neighbors.len(), 3, "window should be filled: {neighbors:?}");
}

/// The lexical leg supplies an older indexed match the recent window misses.
#[test]
fn lexical_index_supplies_older_neighbors() {
    let s = state();
    let older = item(
        "bank-lex",
        "itm-older",
        "alpha unique token",
        "2026-01-01T00:00:00Z",
    );
    let newer = item(
        "bank-lex",
        "itm-newer",
        "beta other thing",
        "2026-01-02T00:00:00Z",
    );
    s.store
        .create_memory_item(&older, "agent")
        .expect("seed older");
    s.store
        .create_memory_item(&newer, "agent")
        .expect("seed newer");
    s.drain_index().expect("drain");
    // Window 1 keeps only the newest item; lexical must add the older match.
    let signals = StoreSignals::with_limits(Arc::clone(&s.store), Arc::clone(&s.index), 1, 8);
    let probe = item(
        "bank-lex",
        "probe",
        "alpha unique token",
        "2026-02-01T00:00:00Z",
    );
    let neighbors = SignalSource::neighbor_texts(&signals, &probe).expect("neighbors");
    assert!(
        neighbors.contains(&candidate_text(&older)),
        "lexical leg missed the older match: {neighbors:?}"
    );
}

/// Belief creates score against stored memory through the same bounded source.
#[test]
fn belief_signal_source_reads_the_bank() {
    let s = state();
    let prior = item(
        "bank-belief",
        "itm-belief",
        "Deploys on Fridays are risky",
        "2026-01-01T00:00:00Z",
    );
    s.store.create_memory_item(&prior, "agent").expect("seed");
    let signals = signals_for(&s);
    let neighbors =
        BeliefSignalSource::neighbor_texts(&signals, "bank-belief", "Deploys on Fridays are risky")
            .expect("belief neighbors");
    assert!(!neighbors.is_empty(), "belief create must see stored rows");
}

/// The stamped `admission_score` is a function of the candidate's content and
/// provenance, not of when the write happened: two candidates with the same
/// text but different `created_at` values score identically.
///
/// Before this contract the recency factor read the wall clock, so the same
/// content written a moment apart stored different last digits.
///
/// Bit-exact comparison is the assertion, not an accident: the invariant is
/// that identical content yields the identical stored bits, which an epsilon
/// cannot check.
#[allow(clippy::float_cmp)]
#[test]
fn admission_score_is_independent_of_the_write_time() {
    let s = state();
    let prior = item(
        "bank-clock",
        "itm-prior",
        "deploys on fridays are risky",
        "2026-01-01T00:00:00Z",
    );
    s.store.create_memory_item(&prior, "agent").expect("seed");
    let signals = signals_for(&s);
    let policy = crate::write_tools::admission_policy(&s, "bank-clock");

    let early = item(
        "bank-clock",
        "itm-early",
        "a genuinely new note about backups",
        "2026-02-01T00:00:00Z",
    );
    let late = item(
        "bank-clock",
        "itm-late",
        "a genuinely new note about backups",
        "2026-02-02T00:00:00Z",
    );

    let early_inputs = clio_admission::signals_for(&early, &signals).expect("early signals");
    let late_inputs = clio_admission::signals_for(&late, &signals).expect("late signals");

    let early_score =
        clio_admission::decide_ungated("task_spec", &early_inputs, &policy).expect("early score");
    let late_score =
        clio_admission::decide_ungated("task_spec", &late_inputs, &policy).expect("late score");
    assert_eq!(
        early_score.admission_score, late_score.admission_score,
        "identical content must score identically regardless of write time"
    );
}

/// The same invariant holds through the real gated write path: two banks that
/// hold the same prior store an identical probe with an identical
/// `admission_score` when the probes' timestamps are a day apart.
#[test]
fn stored_score_is_identical_for_writes_a_day_apart() {
    let s = state();
    let store = |bank: &str, id: &str, gist: &str, ts: &str| {
        crate::store_write::store(
            &s,
            &json!({
                "item": {"id": id, "subject_id": id, "gist": gist},
                "category": "persona",
                "epistemic_kind": "fact",
                "bank": bank,
                "actor": "agent",
                "timestamp": ts,
            }),
        )
        .expect("store")
    };

    // A dissimilar prior in each bank, so only the write time can differ.
    for (bank, suffix) in [("bank-clock-a", "a"), ("bank-clock-b", "b")] {
        let prior = store(
            bank,
            &format!("itm-prior-{suffix}"),
            "deploys on fridays are risky",
            "2026-01-01T00:00:00Z",
        );
        assert!(prior["pass"].as_bool().unwrap_or(false), "{prior}");
    }

    let early = store(
        "bank-clock-a",
        "itm-probe-a",
        "a genuinely new note about backups",
        "2026-02-01T00:00:00Z",
    );
    let late = store(
        "bank-clock-b",
        "itm-probe-b",
        "a genuinely new note about backups",
        "2026-02-02T00:00:00Z",
    );
    assert!(early["pass"].as_bool().unwrap_or(false), "{early}");
    assert!(late["pass"].as_bool().unwrap_or(false), "{late}");
    assert_eq!(
        early["admission_score"], late["admission_score"],
        "the stored score must not depend on the write time"
    );
}
