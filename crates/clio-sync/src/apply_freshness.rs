// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the envelope freshness guard: it decides
//! whether the writer stamp a mutation carries sits too far from this node's
//! clock to be applied, in either direction.
//!
//! ## Owns
//! - The symmetric skew rule over `transaction_time` and one clock reading
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

/// Is the mutation's writer stamp outside the window around the engine's clock?
///
/// The rule is symmetric: a stamp from long ago and a stamp from far in the
/// future are both stale, because either one means this node cannot place the
/// write against its own clock. A tolerance of `0` disables the window, so the
/// check answers `false` for every stamp.
pub(crate) fn is_stale(engine: &ApplyEngine<'_>, transaction_time: &str) -> bool {
    let tolerance = engine.max_envelope_skew_secs;
    if tolerance == 0 {
        return false;
    }
    let Some(written) = parse_utc_epoch(transaction_time) else {
        // A stamp this crate cannot place says nothing about freshness, so the
        // envelope is refused instead of trusted.
        return true;
    };
    (engine.clock)().abs_diff(written) > tolerance
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
/// inside the window: the case decides the outcome, never the day it runs.
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
