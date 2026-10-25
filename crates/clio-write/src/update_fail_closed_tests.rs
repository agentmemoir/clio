// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the fail-closed cases of the `update` rule
//! splitter's discrete path: every call that cannot store its value errors.
//!
//! ## Owns
//! - Already-superseded, unknown-target, blank-value, and belief-target refusals
//! - The no-mutation proof for each refused call
//!
//! ## Does not own
//! - The stored-value and replacement-linkage case (see `update_tests`)
//! - Store persistence cases (see `clio-store` attribute suites)
//!
//! ## Boundary
//!
//! Changes to discrete-path refusal semantics should normally update tests here.
//! Changes to the continuous path should not require changes here.
//!
//! Keep this module focused on the discrete path's refused calls.

use crate::triple::TripleVecSink;
use crate::update::update_tests::{args, item, store, unique};
use crate::update::{UpdateOutcome, update};
use clio_store::Store;
use clio_types::{Actor, Bank, ErrorCode, OpContext, UpdateRule};

/// A discrete update that cannot store its value fails closed instead of
/// reporting success with no replacement.
#[test]
fn t_discrete_update_fails_closed_when_nothing_is_stored() {
    let s = store();
    let bank = unique("b");
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let cont = crate::update::ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ok = args(
        &s,
        &ctx,
        "i1",
        "first",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    assert!(matches!(update(&ok).unwrap(), UpdateOutcome::Discrete(_)));
    // Already closed: nothing is written, so this must not read as success.
    let again = args(
        &s,
        &ctx,
        "i1",
        "second",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    let err = update(&again).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(
        err.message.contains("already superseded"),
        "{}",
        err.message
    );
    // Unknown target fails closed rather than minting an orphan.
    let missing = args(
        &s,
        &ctx,
        "nope",
        "second",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    assert_eq!(update(&missing).unwrap_err().code, ErrorCode::ItemNotFound);
    // A blank value leaves nothing to store.
    s.create_memory_item(&item("i2", &bank), "agent").unwrap();
    let empty_value = args(&s, &ctx, "i2", "  ", UpdateRule::Discrete, &cont, &triple);
    let err = update(&empty_value).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("new_value"), "{}", err.message);
    // A belief's value evolves only through belief_observe.
    let mut belief = item("i3", &bank);
    belief.epistemic_kind = clio_types::EpistemicKind::Belief;
    belief.source_type = Some(clio_types::SourceType::UserStated);
    s.create_memory_item(&belief, "agent").unwrap();
    let denied = args(&s, &ctx, "i3", "no", UpdateRule::Discrete, &cont, &triple);
    let err = update(&denied).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("belief_observe"), "{}", err.message);
    // Every refusal left its target open and emitted no event.
    for id in ["i2", "i3"] {
        let got = s.get_memory_item(&bank, id).unwrap().unwrap();
        assert!(got.tx_until.is_none(), "{id} was closed by a refused call");
    }
    assert_eq!(triple.events().len(), 1, "only the successful call emitted");
}

/// An empty replacement id is a caller bug, not a stored value.
#[test]
fn t_discrete_update_requires_a_replacement_id() {
    let s = store();
    let bank = unique("b");
    s.create_memory_item(&item("i1", &bank), "agent").unwrap();
    let ctx = OpContext::new(Bank::new(&bank), Actor::Agent);
    let cont = crate::update::ContinuousVecSink::default();
    let triple = TripleVecSink::default();
    let mut a = args(
        &s,
        &ctx,
        "i1",
        "value",
        UpdateRule::Discrete,
        &cont,
        &triple,
    );
    a.new_item_id = "";
    let err = update(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("new_item_id"), "{}", err.message);
    let got = s.get_memory_item(&bank, "i1").unwrap().unwrap();
    assert!(got.tx_until.is_none(), "the prior stays open");
}
