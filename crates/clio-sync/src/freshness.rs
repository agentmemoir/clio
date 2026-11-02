// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure age test over an envelope's
//! authenticated `transaction_time`: the stamp reader and the freshness window
//! that says whether a mutation is recent enough to apply.
//!
//! ## Owns
//! - `parse_utc_secs`: the fixed-shape `YYYY-MM-DDTHH:MM:SSZ` stamp reader
//! - `FreshnessPolicy`: the window value, its default, and a disabled form
//! - `Freshness`: the three-way classification (`Fresh`, `Stale`, `Future`)
//!
//! ## Does not own
//! - Reading the clock (`clock` supplies `now_utc`; callers pass `now_secs`)
//! - Applying or rejecting a mutation, and any durable effect of a rejection
//!   (see `apply`)
//! - Ordering between mutations (see `protocol` LWW)
//! - Where the window value is configured (see the caller's config type)
//!
//! ## Boundary
//!
//! Changes to what counts as a well-formed stamp, or to the meaning of the
//! window, should normally require changes here. Changes to what a caller does
//! with a `Stale` or `Future` answer should not.
//!
//! Keep this module pure: no clock reads, no storage, no error type.

use clio_types::{days_from_civil, days_in_month};

/// Default freshness window: seven days, in seconds.
///
/// This is a product choice, not a protocol constant. Seven days tolerates a
/// relay that holds a batch during an outage and a device that syncs weekly,
/// while still refusing a stamp old enough that the row would be a guess.
pub const DEFAULT_FRESHNESS_WINDOW_SECS: i64 = 604_800;

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

/// How an incoming stamp sits against the local clock, given a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Inside the window in both directions.
    Fresh,
    /// Older than `now - window`.
    Stale,
    /// Newer than `now + window`.
    Future,
}

/// The age window applied to an incoming `transaction_time`.
///
/// The test is symmetric because both ends are real failures: an old stamp can
/// be a replay the journal never saw, and a far-future stamp means the writer's
/// clock is wrong enough that the stamp cannot order anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshnessPolicy {
    /// Half-width of the accepted band, in seconds, on each side of now.
    pub window_secs: i64,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            window_secs: DEFAULT_FRESHNESS_WINDOW_SECS,
        }
    }
}

impl FreshnessPolicy {
    /// A policy with no age check. It answers `Fresh` for every stamp.
    #[must_use]
    pub fn disabled() -> Self {
        Self { window_secs: 0 }
    }

    /// Classify an incoming stamp in epoch seconds against a supplied `now`.
    ///
    /// Bounds are inclusive on both ends, so `now - window` and `now + window`
    /// are `Fresh`. A window of zero or less disables the check. Arithmetic
    /// saturates, so extreme inputs cannot overflow.
    #[must_use]
    pub fn classify(&self, incoming_secs: i64, now_secs: i64) -> Freshness {
        if self.window_secs <= 0 {
            return Freshness::Fresh;
        }
        let oldest = now_secs.saturating_sub(self.window_secs);
        let newest = now_secs.saturating_add(self.window_secs);
        if incoming_secs < oldest {
            Freshness::Stale
        } else if incoming_secs > newest {
            Freshness::Future
        } else {
            Freshness::Fresh
        }
    }

    /// Classify a wire stamp as text.
    ///
    /// An unparsable stamp answers `Fresh`, so a malformed `transaction_time`
    /// never opens a new rejection path; shape validation stays with the
    /// existing envelope checks, and a disabled policy short-circuits the same
    /// way.
    #[must_use]
    pub fn classify_text(&self, transaction_time: &str, now_secs: i64) -> Freshness {
        if self.window_secs <= 0 {
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
