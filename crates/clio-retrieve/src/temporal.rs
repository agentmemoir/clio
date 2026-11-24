// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving a supported relative temporal
//! expression in a query into one deterministic half-open window against an
//! explicit caller-supplied anchor.
//!
//! ## Owns
//! - `TemporalWindow` and the supported relative-expression phrase set
//! - Anchor parsing (RFC 3339 with an explicit UTC offset; no server clock)
//! - Window resolution, including the fail-closed unsupported/ambiguous rules
//!
//! ## Does not own
//! - Retrieving candidates in a window (`hybrid`, storage backends)
//! - Fusing the arm by rank (`fusion`, `hybrid_rank`) or binding parameters
//!   (`surface`, MCP/CLI bindings)
//!
//! ## Boundary
//!
//! Changes to the supported expressions or window boundaries should normally
//! require changes here. Changes to how the arm is fused should not require
//! changes here.
//!
//! Keep this module focused on relative temporal-expression resolution.
//!
//! # The anchor is explicit and the rules are fixed
//!
//! Server time is never a hidden input. A caller supplies `query_anchor` as
//! RFC 3339 text with an explicit UTC offset; a missing offset (a bare date or
//! a local time) is rejected instead of guessed. Day boundaries follow the
//! anchor's own offset, so `yesterday` ends at local midnight of the anchor's
//! day; the bounds are then emitted as UTC text.
//!
//! The supported set is deliberately small and fixed:
//!
//! | Phrase | Half-open window (anchor offset) |
//! |--------|----------------------------------|
//! | `today` | `[day start, next day start)` |
//! | `yesterday` | `[previous day start, day start)` |
//! | `this week` | `[ISO week start (Monday), next Monday)` |
//! | `last week` | `[previous Monday, this Monday)` |
//! | `this month` | `[first of month, first of next month)` |
//! | `last month` | `[first of previous month, first of month)` |
//! | `this year` | `[January 1, next January 1)` |
//! | `last year` | `[previous January 1, January 1)` |
//!
//! Weeks start on Monday (ISO 8601); no locale is detected, so the rule cannot
//! drift with a host locale. Any other phrase, a phrase set that resolves to
//! more than one distinct window, or an oversized query yields no window at all:
//! the arm never guesses.

use clio_types::{AmError, ErrorCode};
use jiff::civil::{Date, Time};
use jiff::fmt::temporal::Pieces;
use jiff::tz::{Offset, TimeZone};
use jiff::{Span, ZonedDateTime};

/// Largest query text the parser scans, in UTF-8 bytes.
///
/// A query above this bound contributes no window. The scan below is linear;
/// the bound exists so an unbounded caller string cannot turn the arm into
/// avoidable work.
pub const MAX_TEMPORAL_QUERY_BYTES: usize = 64 * 1024;

/// A relative temporal expression resolved against an explicit anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalWindow {
    /// Inclusive start bound, UTC ISO-8601 text (`%Y-%m-%dT%H:%M:%SZ`).
    pub start: String,
    /// Exclusive end bound, same format and zone as `start`.
    pub end: String,
}

/// One supported relative expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expression {
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    ThisMonth,
    LastMonth,
    ThisYear,
    LastYear,
}

/// Supported phrases in token form. Every phrase is one or two whole tokens,
/// and no phrase is a suffix of another, so a match cannot overlap another.
const PHRASES: &[(&[&str], Expression)] = &[
    (&["today"], Expression::Today),
    (&["yesterday"], Expression::Yesterday),
    (&["this", "week"], Expression::ThisWeek),
    (&["last", "week"], Expression::LastWeek),
    (&["this", "month"], Expression::ThisMonth),
    (&["last", "month"], Expression::LastMonth),
    (&["this", "year"], Expression::ThisYear),
    (&["last", "year"], Expression::LastYear),
];

/// Resolve the relative temporal expression in `query` against `anchor`.
///
/// Returns `Err` when `anchor` is not RFC 3339 text with an explicit UTC
/// offset. Returns `Ok(None)` when the query holds no supported expression or
/// the matched expressions resolve to more than one window; unsupported and
/// ambiguous input never guesses, and a missing anchor is the caller's
/// decision, not this function's (an absent anchor means no arm at all).
///
/// # Errors
/// Malformed `anchor` text (`invalid_argument`).
pub fn resolve_window(query: &str, anchor: &str) -> Result<Option<TemporalWindow>, AmError> {
    let anchor = Pieces::parse(anchor)
        .ok()
        .and_then(|pieces| {
            let offset = pieces.to_numeric_offset()?;
            let time = pieces.time().unwrap_or_else(Time::midnight);
            let local = pieces.date().to_datetime(time);
            TimeZone::fixed(offset).to_zoned_datetime(local).ok()
        })
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::InvalidArgument,
                "query_anchor must be an RFC 3339 timestamp with an explicit UTC offset, \
                 for example 2026-09-26T10:00:00Z or 2026-09-26T10:00:00-07:00",
            )
        })?;
    Ok(find_window(query, &anchor))
}

/// Scan the query's whole tokens for supported phrases and resolve the one
/// window they agree on.
fn find_window(query: &str, anchor: &ZonedDateTime) -> Option<TemporalWindow> {
    if query.len() > MAX_TEMPORAL_QUERY_BYTES {
        return None;
    }
    let tokens: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut windows: Vec<TemporalWindow> = Vec::new();
    for (phrase, expression) in PHRASES {
        let width = phrase.len();
        for start in 0..tokens.len().saturating_sub(width.saturating_sub(1)) {
            let candidate = &tokens[start..start + width];
            if candidate
                .iter()
                .zip(*phrase)
                .all(|(token, word)| token == word)
                && let Some(window) = expression.resolve(anchor)
                && !windows.contains(&window)
            {
                windows.push(window);
            }
        }
    }
    if windows.len() == 1 {
        windows.pop()
    } else {
        None
    }
}

impl Expression {
    /// Resolve this expression to a half-open window in the anchor's offset;
    /// `None` when calendar arithmetic overflows the representable range.
    fn resolve(self, anchor: &ZonedDateTime) -> Option<TemporalWindow> {
        let offset = anchor.offset();
        let day = anchor.date();
        let (start, end): (Date, Date) = match self {
            Self::Today => (day, add_days(day, 1)?),
            Self::Yesterday => (add_days(day, -1)?, day),
            Self::ThisWeek => {
                let monday = add_days(day, -i64::from(day.weekday().to_monday_zero_offset()))?;
                (monday, add_days(monday, 7)?)
            }
            Self::LastWeek => {
                let monday = add_days(day, -i64::from(day.weekday().to_monday_zero_offset()))?;
                (add_days(monday, -7)?, monday)
            }
            Self::ThisMonth => {
                let first = month_start(day.year(), day.month())?;
                (first, next_month(first)?)
            }
            Self::LastMonth => {
                let first = month_start(day.year(), day.month())?;
                (previous_month(first)?, first)
            }
            Self::ThisYear => (
                Date::new(day.year(), 1, 1).ok()?,
                Date::new(day.year() + 1, 1, 1).ok()?,
            ),
            Self::LastYear => (
                Date::new(day.year() - 1, 1, 1).ok()?,
                Date::new(day.year(), 1, 1).ok()?,
            ),
        };
        Some(TemporalWindow {
            start: utc_text(offset, start),
            end: utc_text(offset, end),
        })
    }
}

fn add_days(date: Date, days: i64) -> Option<Date> {
    date.checked_add(Span::new().days(days)).ok()
}

fn month_start(year: i16, month: i8) -> Option<Date> {
    Date::new(year, month, 1).ok()
}

fn next_month(first: Date) -> Option<Date> {
    let (year, month) = (first.year(), first.month());
    if month == 12 {
        Date::new(year + 1, 1, 1).ok()
    } else {
        Date::new(year, month + 1, 1).ok()
    }
}

fn previous_month(first: Date) -> Option<Date> {
    let (year, month) = (first.year(), first.month());
    if month == 1 {
        Date::new(year - 1, 12, 1).ok()
    } else {
        Date::new(year, month - 1, 1).ok()
    }
}

/// Midnight of `date` in `offset`, emitted as UTC `%Y-%m-%dT%H:%M:%SZ` text.
fn utc_text(offset: Offset, date: Date) -> String {
    offset
        .to_timestamp(date.at(0, 0, 0, 0))
        .expect("a fixed offset maps each local time exactly once")
        .to_string()
}

#[cfg(test)]
#[path = "temporal_tests.rs"]
mod temporal_tests;

#[cfg(test)]
#[path = "temporal_arm_tests.rs"]
mod temporal_arm_tests;
