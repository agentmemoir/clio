// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure age test over an envelope's
//! authenticated `transaction_time`: the stamp reader and the freshness bounds
//! that say whether a mutation is recent enough to apply.
//!
//! ## Owns
//! - `parse_utc_secs`: the fixed-shape `YYYY-MM-DDTHH:MM:SSZ` stamp reader
//! - `FreshnessPolicy`: the two one-sided bounds, their defaults, and a
//!   disabled form
//! - `Freshness`: the three-way classification (`Fresh`, `Stale`, `Future`)
//!
//! ## Does not own
//! - Reading the clock (`clock` supplies `now_utc`; callers pass `now_secs`)
//! - Applying or rejecting a mutation, and any durable effect of a rejection
//!   (see `apply`)
//! - Ordering between mutations (see `protocol` LWW)
//! - Where the bounds are configured (see the caller's config type)
//!
//! ## Boundary
//!
//! Changes to what counts as a well-formed stamp, or to the meaning of the
//! bounds, should normally require changes here. Changes to what a caller does
//! with a `Stale` or `Future` answer should not.
//!
//! Keep this module pure: no clock reads, no storage, no error type.

use clio_types::{days_from_civil, days_in_month};

/// Default allowance for an old stamp: 0 seconds, so the past side ships off.
///
/// Absolute age is not a usable bound here. `transaction_time` is the origin
/// device's own write stamp, and push and pull both deliver stamps unchanged,
/// so a device that was offline for weeks, or a new device reading the whole
/// journal, sends a backlog whose stamps are arbitrarily old. A rejection
/// dead-letters the row and freezes the pull cursor for the bank, so an age
/// bound would stall sync on legitimate traffic.
///
/// The field stays, so a deployment that does bound its backlog can set its own
/// limit. Only the default is off.
pub const DEFAULT_FRESHNESS_MAX_AGE_SECS: i64 = 0;

/// Default allowance for a future-dated stamp: 0 seconds, so the future side
/// ships off.
///
/// Some entity kinds do not stamp the envelope with the change time. A triple
/// ships its transaction-time close, or its valid-time close when no
/// transaction-time close exists, and a non-upsert item operation ships the
/// row's stored transaction or valid time. A valid time is world time: a row
/// that is valid for years ahead carries a stamp years ahead, and the change
/// that shipped it happened now. Refusing such a stamp would dead-letter a
/// legitimate row, freeze the bank's pull cursor, and set that kind's ordering
/// mark to the same future value, so every later legitimate row of that kind
/// would compare below it and be refused too.
///
/// The field stays, so a deployment whose stamps are known to be change times
/// can set its own lead. Only the default is off.
pub const DEFAULT_FRESHNESS_MAX_FUTURE_SECS: i64 = 0;

/// Length of the one accepted stamp shape, `YYYY-MM-DDTHH:MM:SSZ`.
const STAMP_LEN: usize = 20;

/// Seconds in one day.
const SECS_PER_DAY: i64 = 86_400;

/// Parse exactly `YYYY-MM-DDTHH:MM:SSZ` into whole Unix epoch seconds.
///
/// The shape is fixed at 20 ASCII characters: four year digits, a dash, two
/// month digits, a dash, two day digits, `T`, two hour digits, a colon, two
/// minute digits, a colon, two second digits, then `Z`. Anything else returns
/// `None`: a missing `Z`, a space separator, a fractional-second tail, an
/// offset such as `+00:00`, a short string, or an empty one.
///
/// The reader checks the calendar fields too, so `2024-02-29` parses and
/// `2023-02-29` does not. Day conversion is `clio_types::days_from_civil`, the
/// workspace's single copy of that arithmetic.
#[must_use]
pub fn parse_utc_secs(text: &str) -> Option<i64> {
    if text.len() != STAMP_LEN || !text.is_ascii() {
        return None;
    }
    let b = text.as_bytes();
    let shape_ok = b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'Z';
    if !shape_ok {
        return None;
    }
    let year = i64::from(digits(b, 0, 4)?);
    let month = digits(b, 5, 2)?;
    let day = digits(b, 8, 2)?;
    let hour = i64::from(digits(b, 11, 2)?);
    let minute = i64::from(digits(b, 14, 2)?);
    let second = i64::from(digits(b, 17, 2)?);
    let calendar_ok = (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
        && hour <= 23
        && minute <= 59
        && second <= 59;
    if !calendar_ok {
        return None;
    }
    let secs_of_day = hour * 3_600 + minute * 60 + second;
    Some(days_from_civil(year, month, day) * SECS_PER_DAY + secs_of_day)
}

/// Read `count` ASCII decimal digits starting at `start`, or `None`.
fn digits(bytes: &[u8], start: usize, count: usize) -> Option<u32> {
    let mut value: u32 = 0;
    for offset in 0..count {
        let byte = *bytes.get(start + offset)?;
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(byte - b'0');
    }
    Some(value)
}

/// How an incoming stamp sits against the local clock, given two bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Inside both bounds.
    Fresh,
    /// Older than `now - max_age`.
    Stale,
    /// Newer than `now + max_future`.
    Future,
}

/// The age bounds applied to an incoming `transaction_time`.
///
/// Both sides ship off, for different reasons, and both stay available for a
/// deployment whose stamps are known to be change times. A bound of 0 or less
/// disables only its own side and leaves the other one working.
///
/// The past side ships off because a legitimate sync backlog can be arbitrarily
/// old: push and pull both deliver the origin stamp unchanged, so an offline
/// device, or a new device reading the whole journal, sends old stamps.
///
/// The future side ships off because not every kind stamps the envelope with
/// the change time. A triple ships a transaction-time or valid-time close, and
/// a non-upsert item operation ships a stored transaction or valid time. A
/// valid time is world time, so a legitimate row can carry a stamp in the
/// future. A refusal would dead-letter that row and freeze the bank's pull
/// cursor.
///
/// The per-device ordering mark, not either wall-clock bound, is the defence
/// against a replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshnessPolicy {
    /// How old a stamp may be, in seconds. 0 or less disables this side.
    pub max_age_secs: i64,
    /// How far ahead of now a stamp may sit, in seconds. 0 or less disables
    /// this side.
    pub max_future_secs: i64,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            max_age_secs: DEFAULT_FRESHNESS_MAX_AGE_SECS,
            max_future_secs: DEFAULT_FRESHNESS_MAX_FUTURE_SECS,
        }
    }
}

impl FreshnessPolicy {
    /// A policy with no age check. It answers `Fresh` for every stamp.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            max_age_secs: 0,
            max_future_secs: 0,
        }
    }

    /// True when neither side can reject anything.
    fn is_disabled(&self) -> bool {
        self.max_age_secs <= 0 && self.max_future_secs <= 0
    }

    /// Classify an incoming stamp in epoch seconds against a supplied `now`.
    ///
    /// Bounds are inclusive on both ends, so `now - max_age` and `now +
    /// max_future` are `Fresh`. A bound of zero or less disables only its own
    /// side: a disabled past side never returns `Stale`, and a disabled future
    /// side never returns `Future`. Arithmetic saturates, so extreme inputs
    /// cannot overflow.
    #[must_use]
    pub fn classify(&self, incoming_secs: i64, now_secs: i64) -> Freshness {
        if self.max_age_secs > 0 && incoming_secs < now_secs.saturating_sub(self.max_age_secs) {
            return Freshness::Stale;
        }
        if self.max_future_secs > 0 && incoming_secs > now_secs.saturating_add(self.max_future_secs)
        {
            return Freshness::Future;
        }
        Freshness::Fresh
    }

    /// Classify a wire stamp as text.
    ///
    /// An unparsable stamp answers `Fresh`, so a malformed `transaction_time`
    /// never opens a new rejection path; shape validation stays with the
    /// existing envelope checks, and a disabled policy short-circuits the same
    /// way.
    #[must_use]
    pub fn classify_text(&self, transaction_time: &str, now_secs: i64) -> Freshness {
        if self.is_disabled() {
            return Freshness::Fresh;
        }
        match parse_utc_secs(transaction_time) {
            Some(incoming_secs) => self.classify(incoming_secs, now_secs),
            None => Freshness::Fresh,
        }
    }
}

#[cfg(test)]
#[path = "freshness_tests.rs"]
mod tests;
