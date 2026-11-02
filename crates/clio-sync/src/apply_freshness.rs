// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the envelope freshness guard: it decides
//! whether the writer stamp a mutation carries is too old to apply here, and
//! whether it sits implausibly far in the future.
//!
//! ## Owns
//! - The past-directed age bound over `transaction_time` and one clock reading
//! - The separate, larger future allowance that protects LWW integrity
//! - The disabled window (`0` tolerance checks nothing)
//! - The refusal of a stamp the reader cannot place
//!
//! ## Does not own
//! - Clock reads and stamp parsing (see `clock`)
//! - The guard's place in the apply order (see `apply`)
//! - What a stale envelope means for a cursor (see `client`, `server_push`)
//!
//! ## Boundary
//!
//! Changes to the freshness window should normally require changes here.
//! Changes to the stamp format or the clock should not require changes here.
//!
//! Keep this module focused on the freshness decision.

use crate::apply::ApplyEngine;
use crate::clock::parse_utc_epoch;
#[cfg(test)]
use clio_store::{DekProvider, Store, SyncStore};

/// How far ahead of this node's clock a writer stamp may sit, in seconds.
///
/// This is not a replay bound. A relay can only replay a row that already
/// exists, so a replay's stamp is always in the past, and the past bound is the
/// only rule that answers replay. The future direction protects LWW integrity
/// instead: a stamp ahead of real time wins every comparison against the writes
/// that follow it until real time catches up. The allowance is larger than any
/// sensible past bound so a peer whose clock runs fast keeps its data, and an
/// hour bounds the damage a badly set clock can do to the merge order.
pub(crate) const ENVELOPE_FUTURE_ALLOWANCE_SECS: u64 = 3_600;

/// Is the mutation's writer stamp outside the bounds this engine allows?
///
/// Two bounds apply, and they answer different questions. A stamp too far in
/// the past is refused as an age bound: `max_envelope_skew_secs` seconds, which
/// is the operator setting. A stamp too far in the future is refused by
/// [`ENVELOPE_FUTURE_ALLOWANCE_SECS`], which keeps a fast clock from reordering
/// the merge. A past bound of `0` disables both, so the check answers `false`
/// for every stamp.
pub(crate) fn is_stale(engine: &ApplyEngine<'_>, transaction_time: &str) -> bool {
    let max_age = engine.max_envelope_skew_secs;
    if max_age == 0 {
        return false;
    }
    let Some(written) = parse_utc_epoch(transaction_time) else {
        // A stamp this crate cannot place says nothing about freshness, so the
        // envelope is refused instead of trusted.
        return true;
    };
    let now = (engine.clock)();
    let max_age = i64::try_from(max_age).unwrap_or(i64::MAX);
    let max_ahead = i64::try_from(ENVELOPE_FUTURE_ALLOWANCE_SECS).unwrap_or(i64::MAX);
    // `saturating_sub` keeps an extreme clock reading from overflowing: either
    // difference is then far larger than any bound and the stamp is refused.
    now.saturating_sub(written) > max_age || written.saturating_sub(now) > max_ahead
}

/// A clock seam pinned to one epoch second, for tests.
///
/// The value is leaked so an engine can borrow it for a whole case: the seam
/// type stays exactly the one production uses, and a test process leaks a few
/// words.
#[cfg(test)]
pub(crate) fn pinned(epoch: i64) -> &'static dyn Fn() -> i64 {
    Box::leak(Box::new(move || epoch))
}

/// A clock seam pinned to the second `stamp` names.
#[cfg(test)]
pub(crate) fn pinned_at(stamp: &str) -> &'static dyn Fn() -> i64 {
    pinned(parse_utc_epoch(stamp).unwrap_or(0))
}

/// An apply engine for tests whose clock reads `stamp`.
///
/// Fixing the clock to the fixture's own stamp keeps every fixed-date fixture
/// at zero age, so the case decides the outcome, never the day it runs. The age
/// bound is the shipped default, which bounds nothing.
#[cfg(test)]
pub(crate) fn test_engine<'a>(
    store: &'a dyn Store,
    sync: &'a dyn SyncStore,
    device_id: &str,
    kms: Option<&'a dyn DekProvider>,
    stamp: &str,
) -> ApplyEngine<'a> {
    ApplyEngine {
        store,
        sync,
        device_id: device_id.to_owned(),
        kms,
        clock: pinned_at(stamp),
        max_envelope_skew_secs: crate::ENVELOPE_SKEW_DEFAULT_SECS,
    }
}

#[cfg(test)]
#[path = "apply_freshness_tests.rs"]
mod apply_freshness_tests;
