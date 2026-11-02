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
//! - The refusal of a stamp the reader cannot place, and the documented legacy
//!   stamp forms this reader places instead of refusing
//!
//! ## Does not own
//! - Clock reads and the strict wire-form parse (see `clock`)
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

/// Epoch seconds for a writer stamp in any form this store writes, or `None`
/// when the stamp names no instant this build can place.
///
/// The wire form is strict (`YYYY-MM-DDTHH:MM:SSZ`). The store also stamps item
/// rows as epoch milliseconds and as `YYYY-MM-DD[THH:MM:SS[.fff]][Z]`, and an
/// item's stored stamp travels unchanged as a mutation's `transaction_time`, so
/// those documented forms are placed here too instead of being refused as old.
/// A fraction is dropped (the comparison works in whole seconds). An offset
/// zone, a spelled month, or junk stays `None`, and the caller records it as a
/// malformed stamp rather than calling it stale.
pub(crate) fn writer_stamp_epoch(stamp: &str) -> Option<i64> {
    let stamp = stamp.trim();
    if let Some(secs) = parse_utc_epoch(stamp) {
        return Some(secs);
    }
    // Epoch milliseconds, optionally signed: all digits is the whole test,
    // because a date also starts with a digit.
    let digits = stamp.strip_prefix('-').unwrap_or(stamp);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return stamp
            .parse::<i64>()
            .ok()
            .map(|millis| millis.div_euclid(1_000));
    }
    let trimmed = stamp.strip_suffix('Z').unwrap_or(stamp);
    let (whole, fraction) = match trimmed.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (trimmed, None),
    };
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let normalized = match whole.len() {
        10 => format!("{whole}T00:00:00Z"),
        19 if whole.as_bytes()[10] == b' ' => {
            format!("{}T{}Z", &whole[..10], &whole[11..])
        }
        19 => format!("{whole}Z"),
        _ => return None,
    };
    parse_utc_epoch(&normalized)
}

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
    let Some(written) = writer_stamp_epoch(transaction_time) else {
        // A stamp this crate cannot place says nothing about freshness, so the
        // envelope is refused instead of trusted. The refusal records the
        // stamp as malformed, not as old.
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

/// An apply engine for tests whose clock reads `stamp`.
///
/// Fixing the clock to the fixture's own stamp keeps every fixed-date fixture
/// at zero age, so the case decides the outcome, never the day it runs. The age
/// bound is the shipped default, which bounds nothing. A stamp this helper
/// cannot place is a fixture defect and panics: silently pinning the clock to
/// the epoch would let a malformed fixture pass by taking a bound branch.
#[cfg(test)]
pub(crate) fn test_engine<'a>(
    store: &'a dyn Store,
    sync: &'a dyn SyncStore,
    device_id: &str,
    kms: Option<&'a dyn DekProvider>,
    stamp: &str,
) -> ApplyEngine<'a> {
    let now = writer_stamp_epoch(stamp)
        .unwrap_or_else(|| panic!("test engine stamp {stamp:?} names no instant"));
    test_engine_at(store, sync, device_id, kms, now)
}

/// An apply engine for tests whose clock is pinned to an explicit epoch second.
/// A case whose fixture stamp is deliberately unplaceable uses this, so the
/// fixture defect guard above still holds for every ordinary fixture.
#[cfg(test)]
pub(crate) fn test_engine_at<'a>(
    store: &'a dyn Store,
    sync: &'a dyn SyncStore,
    device_id: &str,
    kms: Option<&'a dyn DekProvider>,
    now: i64,
) -> ApplyEngine<'a> {
    ApplyEngine {
        store,
        sync,
        device_id: device_id.to_owned(),
        kms,
        clock: pinned(now),
        max_envelope_skew_secs: crate::ENVELOPE_SKEW_DEFAULT_SECS,
    }
}

#[cfg(test)]
#[path = "apply_freshness_tests.rs"]
mod apply_freshness_tests;
