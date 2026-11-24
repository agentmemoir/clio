// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests which banks one push locks before it applies anything: the
//! envelope bank, every bank a mutation names, and every bank a denial carrier
//! covers, sorted and deduplicated so every push takes its locks in one global
//! order.
//!
//! ## Owns
//! - The lock set one push computes from its envelope bank and its mutations
//!
//! ## Does not own
//! - The push route's behavior over a running server (`server_push_denial_tests`
//!   and `server_push_stale_tests`)
//! - The apply lock map itself (`server`)
//!
//! ## Boundary
//!
//! These tests break when the set of banks a push locks changes; they never
//! change it.
//!
//! Keep this module focused on the push route's lock set.

use super::banks_to_lock;
use crate::apply_fixtures::{env, item_value};
use crate::denial::{DenialCarrier, denial_mutation};
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncMutation};

/// A stamp every fixture in this module shares; nothing here reads a clock.
const STAMP: &str = "2026-01-01T00:00:00Z";

/// One item mutation whose own bank is `bank`, built the way the other push
/// tests build theirs.
fn item_op(mid: &str, bank: &str) -> SyncMutation {
    env(
        mid,
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        STAMP,
        "dev-a",
        item_value(bank, "i-1", "s-1", STAMP, "gist"),
    )
}

/// One denial operation whose covered banks are `banks`, the shape the push
/// route reads as a carrier.
fn carrier_op(bank: &str, request_ref: &str, banks: &[&str]) -> SyncMutation {
    denial_mutation(
        bank,
        "s-held",
        &DenialCarrier::new(request_ref, banks),
        "dev-a",
        STAMP,
    )
}

/// A mutation that names the envelope bank asks for that one bank.
#[test]
fn envelope_only_when_every_mutation_stays_in_the_envelope_bank() {
    let mutations = vec![item_op("m-1", "a")];
    assert_eq!(banks_to_lock("a", &mutations), vec!["a".to_owned()]);
}

/// The mutation bank is locked even though the envelope names another bank:
/// the item row the mutation writes lives in the mutation's bank, so a push
/// that left it unlocked could race another push on that same row.
#[test]
fn locks_the_bank_a_mutation_writes_not_only_the_envelope_bank() {
    let mutations = vec![item_op("m-1", "b")];
    assert_eq!(
        banks_to_lock("a", &mutations),
        vec!["a".to_owned(), "b".to_owned()]
    );
}

/// A repeated bank costs one lock and the set comes back sorted, so every push
/// acquires its locks in the same global order.
#[test]
fn dedupes_and_sorts_the_banks_it_locks() {
    let mutations = vec![
        item_op("m-1", "a"),
        item_op("m-2", "b"),
        item_op("m-3", "a"),
    ];
    assert_eq!(
        banks_to_lock("b", &mutations),
        vec!["a".to_owned(), "b".to_owned()]
    );
}

/// A denial carrier writes a serve marker for every bank it covers, so the
/// covered bank is locked even when no mutation names it.
#[test]
fn locks_every_covered_bank_of_a_denial_carrier() {
    let mutations = vec![carrier_op("a", "req-1", &["c"])];
    assert_eq!(
        banks_to_lock("a", &mutations),
        vec!["a".to_owned(), "c".to_owned()]
    );
}

/// A carrier covering several banks, one of them the envelope bank, still
/// yields one sorted entry per bank.
#[test]
fn a_carrier_covering_the_envelope_bank_adds_only_the_other_banks() {
    let mutations = vec![carrier_op("a", "req-2", &["d", "a", "c"])];
    assert_eq!(
        banks_to_lock("a", &mutations),
        vec!["a".to_owned(), "c".to_owned(), "d".to_owned()]
    );
}
