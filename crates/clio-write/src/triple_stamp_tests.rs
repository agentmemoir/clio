// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing that caller-supplied triple time axes
//! reach the stored edge and carrier item in canonical form, and that a value the
//! store cannot place is refused with nothing written.
//!
//! ## Owns
//! - Canonicalisation assertions over `triple_add` for `valid_from`,
//!   `valid_until`, and the transaction stamp
//! - The refusal assertion for an unplaceable `valid_from` or `valid_until`
//! - `triple_end`'s canonical `valid_until`
//!
//! ## Does not own
//! - Admission, supersession, or telemetry behaviour (see `triple_tests`)
//! - The store's own stamp reader (see `clio-store`)
//!
//! ## Boundary
//!
//! Changes to how a triple time axis is canonicalised or refused should normally
//! require changes here. Changes to admission or interval algebra should not.
//!
//! Keep this module focused on the triple time axes.

use crate::triple::{TripleAddArgs, TripleVecSink, triple_add, triple_end};
use clio_admission::{AdmissionPolicy, RankingEnv, SignalSource, VecSink};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store};
use clio_types::{
    Actor, AmError, Bank, EpistemicKind, ErrorCode, MemoryItem, OpContext, TimeAxis, TripleQuery,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bank-id suffix so parallel tests never share a bank.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{sequence}", std::process::id())
}

#[derive(Debug, Default)]
struct EmptySignals;

impl SignalSource for EmptySignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

fn store() -> SqliteStore {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    SqliteStore::open_in_memory(kms).expect("sqlite")
}

/// One `triple_add` over a fresh bank, returning the committed edge id.
fn add(
    s: &SqliteStore,
    ctx: &OpContext,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
    timestamp: &str,
) -> Result<String, AmError> {
    let env = RankingEnv::default();
    let admit = VecSink::default();
    let trip = TripleVecSink::default();
    let r = triple_add(
        s,
        &EmptySignals,
        &TripleAddArgs {
            ctx,
            subject: "Alice",
            predicate: "lives_in",
            object: "Boston",
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: Some(0.9),
            valid_from,
            valid_until,
            supersede: true,
            snapshot_ref: None,
            gist_ref: None,
            context: None,
            update_rule: None,
            subject_id: "subj",
            policy: AdmissionPolicy::new(&env),
            admit_sink: &admit,
            triple_sink: &trip,
            timestamp,
            dry_run: false,
        },
    )?;
    let edge_id = r
        .outcome
        .as_ref()
        .expect("an admitted triple commits")
        .new_edge_id
        .clone();
    Ok(edge_id)
}

#[test]
fn a_fractional_valid_from_is_stored_as_the_same_second() {
    let s = store();
    let bank = unique("b-stamp-frac");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let edge_id = add(
        &s,
        &ctx,
        Some("2026-03-04T05:06:07.987654Z"),
        None,
        "2026-03-04T05:06:07.987654Z",
    )
    .expect("a fractional stamp is placeable");
    let edge = s
        .get_triple(&bank, &edge_id)
        .expect("read edge")
        .expect("stored edge");
    assert_eq!(edge.valid_from, "2026-03-04T05:06:07Z");
    assert_eq!(edge.tx_from, "2026-03-04T05:06:07Z");
    assert_eq!(edge.created_at, "2026-03-04T05:06:07Z");
    let item = s
        .get_memory_item(&bank, &edge.item_id)
        .expect("read item")
        .expect("stored carrier");
    assert_eq!(item.valid_from, "2026-03-04T05:06:07Z");
    assert_eq!(item.updated_at, "2026-03-04T05:06:07Z");
}

#[test]
fn a_fractional_valid_until_is_stored_as_the_same_second() {
    let s = store();
    let bank = unique("b-stamp-until-frac");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let edge_id = add(
        &s,
        &ctx,
        Some("2026-03-04T05:06:07Z"),
        Some("2026-03-05T08:09:10.250Z"),
        "2026-03-04T05:06:07Z",
    )
    .expect("a fractional bound is placeable");
    let edge = s
        .get_triple(&bank, &edge_id)
        .expect("read edge")
        .expect("stored edge");
    assert_eq!(edge.valid_until.as_deref(), Some("2026-03-05T08:09:10Z"));
}

#[test]
fn an_offset_valid_from_is_stored_as_the_equivalent_utc_instant() {
    let s = store();
    let bank = unique("b-stamp-offset");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let edge_id = add(
        &s,
        &ctx,
        Some("2026-03-04T05:06:07+02:00"),
        Some("2026-03-04T05:06:07-05:30"),
        "2026-03-04T05:06:07+02:00",
    )
    .expect("an offset stamp is placeable");
    let edge = s
        .get_triple(&bank, &edge_id)
        .expect("read edge")
        .expect("stored edge");
    assert_eq!(edge.valid_from, "2026-03-04T03:06:07Z");
    assert_eq!(edge.valid_until.as_deref(), Some("2026-03-04T10:36:07Z"));
    assert_eq!(edge.tx_from, "2026-03-04T03:06:07Z");
}

#[test]
fn an_unplaceable_valid_from_is_refused_and_stores_nothing() {
    let s = store();
    let bank = unique("b-stamp-bad");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let err = add(
        &s,
        &ctx,
        Some("2026-02-30T00:00:00Z"),
        None,
        "2026-03-04T05:06:07Z",
    )
    .expect_err("a day February does not have is not a stamp");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    assert!(err.message.contains("2026-02-30"), "{err}");
    assert!(
        s.get_triple(&bank, "trp").expect("read").is_none(),
        "a refused add left a row"
    );
    let rows = s.list_triples(&bank).expect("list");
    assert!(rows.is_empty(), "a refused add left rows: {rows:?}");
}

#[test]
fn an_unplaceable_valid_until_is_refused_and_stores_nothing() {
    let s = store();
    let bank = unique("b-stamp-bad-until");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let err = add(
        &s,
        &ctx,
        Some("2026-03-04T05:06:07Z"),
        Some("2026-03-04"),
        "2026-03-04T05:06:07Z",
    )
    .expect_err("a date without a clock is not a stamp this build places");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    let rows = s.list_triples(&bank).expect("list");
    assert!(rows.is_empty(), "a refused add left rows: {rows:?}");
}

#[test]
fn an_absent_valid_until_stays_absent() {
    let s = store();
    let bank = unique("b-stamp-open");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let edge_id = add(&s, &ctx, None, None, "2026-03-04T05:06:07Z").expect("open triple");
    let edge = s
        .get_triple(&bank, &edge_id)
        .expect("read edge")
        .expect("stored edge");
    assert_eq!(edge.valid_until, None, "absent must not become a string");
    assert!(edge.is_open());
    let item = s
        .get_memory_item(&bank, &edge.item_id)
        .expect("read item")
        .expect("stored carrier");
    assert_eq!(item.valid_until, None);
    // An empty value is the caller convention for absent, not a stored string.
    let empty_id =
        add(&s, &ctx, None, Some(""), "2026-03-04T05:06:08Z").expect("an empty bound means absent");
    let empty = s
        .get_triple(&bank, &empty_id)
        .expect("read edge")
        .expect("stored edge");
    assert_eq!(empty.valid_until, None);
}

#[test]
fn triple_end_stores_a_canonical_valid_until() {
    let s = store();
    let bank = unique("b-stamp-end");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let edge_id = add(&s, &ctx, None, None, "2026-03-04T05:06:07Z").expect("open triple");
    let trip = TripleVecSink::default();
    triple_end(
        &s,
        &ctx,
        "Alice",
        "lives_in",
        None,
        "2026-04-05T06:07:08.750+02:00",
        "2026-04-05T06:07:08.750+02:00",
        None,
        &trip,
    )
    .expect("a fractional, offset bound is placeable");
    let edge = s
        .get_triple(&bank, &edge_id)
        .expect("read edge")
        .expect("stored edge");
    assert_eq!(edge.valid_until.as_deref(), Some("2026-04-05T04:07:08Z"));
    assert!(!edge.is_open());
}

#[test]
fn a_close_in_the_second_the_edge_started_keeps_it_in_transaction_history() {
    let s = store();
    let bank = unique("b-stamp-same-second");
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Agent);
    let edge_id = add(&s, &ctx, None, None, "2026-03-04T05:06:07Z").expect("open triple");
    let trip = TripleVecSink::default();
    triple_end(
        &s,
        &ctx,
        "Alice",
        "lives_in",
        None,
        "2026-03-04T05:06:07Z",
        "2026-03-04T05:06:07Z",
        None,
        &trip,
    )
    .expect("a close in the starting second is placeable");
    let edge = s
        .get_triple(&bank, &edge_id)
        .expect("read edge")
        .expect("stored edge");
    assert_eq!(edge.tx_from, "2026-03-04T05:06:07Z");
    assert_eq!(
        edge.tx_until.as_deref(),
        Some("2026-03-04T05:06:08Z"),
        "a close on the edge's own start must widen, or the interval is empty"
    );
    assert!(
        edge.tx_until.as_deref().unwrap() > edge.tx_from.as_str(),
        "transaction intervals are compared as text"
    );
    let at_start = s
        .query_triples(&TripleQuery {
            bank_id: bank.clone(),
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some("2026-03-04T05:06:07Z".into()),
            time_axis: TimeAxis::Transaction,
        })
        .expect("query at the starting second");
    assert_eq!(
        at_start.len(),
        1,
        "the closed edge left transaction history: {at_start:?}"
    );
    let at_close = s
        .query_triples(&TripleQuery {
            bank_id: bank,
            subject: Some("Alice".into()),
            predicate: Some("lives_in".into()),
            object: None,
            as_of: Some("2026-03-04T05:06:08Z".into()),
            time_axis: TimeAxis::Transaction,
        })
        .expect("query at the closing second");
    assert!(at_close.is_empty(), "{at_close:?}");
}
