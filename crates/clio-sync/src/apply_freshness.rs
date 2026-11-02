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
//! - The instant-aware order of two writer stamps carried in different
//!   encodings
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
use crate::clock::{format_utc, parse_utc_epoch};
#[cfg(test)]
use clio_store::{DekProvider, Store, SyncStore};
use clio_types::{AmError, ErrorCode};

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
/// A fraction is dropped here, because the age bound works in whole seconds;
/// the stamp order keeps it (see [`writer_stamp_millis`]). An offset zone, a
/// spelled month, or junk stays `None`, and the caller records it as a
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

/// Order two writer stamps by the instant each names, not by its text.
///
/// One field carries stamps in more than one encoding, so text order can put a
/// later instant below an earlier one: `1793597865118` (epoch millis) sorts
/// before `2026-01-01T00:00:00Z` although it names the later time. When both
/// sides place, the instants decide, at the finest precision either side
/// carries (see [`writer_stamp_millis`]), so two writes inside one second still
/// order by the instant each names instead of tying. A pair with a side this
/// reader cannot place has no two instants to compare, so it falls back to text
/// and keeps the order every caller had before.
pub(crate) fn writer_stamp_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (writer_stamp_millis(a), writer_stamp_millis(b)) {
        (Some(a_instant), Some(b_instant)) => a_instant.cmp(&b_instant),
        _ => a.cmp(b),
    }
}

/// Epoch milliseconds for a writer stamp, at the finest precision the stamp
/// carries, or `None` when the stamp names no instant this build can place.
///
/// A stamp with no fraction names a whole second, so its fraction is zero; an
/// epoch-millis stamp is already milliseconds and is taken as written. The
/// fraction is truncated, not rounded, to three digits.
pub(crate) fn writer_stamp_millis(stamp: &str) -> Option<i64> {
    let stamp = stamp.trim();
    let digits = stamp.strip_prefix('-').unwrap_or(stamp);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return stamp.parse::<i64>().ok();
    }
    let secs = writer_stamp_epoch(stamp)?;
    let whole = stamp.strip_suffix('Z').unwrap_or(stamp);
    let Some((_, fraction)) = whole.split_once('.') else {
        return secs.checked_mul(1_000);
    };
    if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut millis = 0i64;
    for byte in fraction.bytes().take(3) {
        millis = millis * 10 + i64::from(byte - b'0');
    }
    for _ in fraction.len().min(3)..3 {
        millis *= 10;
    }
    secs.checked_mul(1_000)?.checked_add(millis)
}

/// The one encoding a writer stamp leaves this node in: canonical ISO-8601 UTC
/// at whole seconds, or the refusal of a stamp no clock reading can place.
///
/// The store stamps rows in more than one legend (epoch millis from `now_iso`,
/// bare dates, ISO with a fraction or an offset zone), and a row's stored stamp
/// travels unchanged as a mutation's `transaction_time`. Every writer puts the
/// same second on the wire so a peer never compares two legends. Placement is
/// the store's own canonicaliser first, because it is what every write path
/// already stores, and then the legacy forms this module places for freshness.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] naming the value that cannot be placed: a
/// stamp that names no instant must be refused, never copied to the wire.
pub(crate) fn canonical_writer_stamp(raw: &str) -> Result<String, AmError> {
    if let Some(canonical) = clio_store::canonical_stamp(raw) {
        return Ok(canonical);
    }
    match writer_stamp_epoch(raw) {
        Some(secs) => Ok(format_utc(secs)),
        None => Err(refused_stamp(raw)),
    }
}

/// The frame around the value in [`refused_stamp`]'s sentence, in one place so
/// the writer and the reader beside it cannot drift apart.
const REFUSAL_BEFORE: &str = "transaction_time `";
const REFUSAL_AFTER: &str = "` is not a timestamp this build can place";

/// The refusal [`canonical_writer_stamp`] raises for a stamp no clock reading
/// can place: `InvalidArgument` naming the field and the value.
fn refused_stamp(raw: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!("{REFUSAL_BEFORE}{raw}{REFUSAL_AFTER}"),
    )
}

/// The value named by [`canonical_writer_stamp`]'s refusal, or `None` when
/// `err` is any other failure.
///
/// A packaging caller must contain that one refusal — hold the row's own kind
/// and report it — without containing a storage or seal failure, and a seal
/// failure also reports `InvalidArgument`. Recognising the sentence this module
/// alone writes is what separates the two, so the reader sits beside the writer.
pub(crate) fn unplaceable_stamp_value(err: &AmError) -> Option<&str> {
    if err.code != ErrorCode::InvalidArgument {
        return None;
    }
    err.message
        .strip_prefix(REFUSAL_BEFORE)?
        .strip_suffix(REFUSAL_AFTER)
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
