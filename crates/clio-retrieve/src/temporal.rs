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

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone};
use clio_types::{AmError, ErrorCode};

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
    let anchor = DateTime::parse_from_rfc3339(anchor).map_err(|_| {
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
fn find_window(query: &str, anchor: &DateTime<FixedOffset>) -> Option<TemporalWindow> {
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
    fn resolve(self, anchor: &DateTime<FixedOffset>) -> Option<TemporalWindow> {
        let offset = *anchor.offset();
        let day = anchor.date_naive();
        let (start, end): (NaiveDate, NaiveDate) = match self {
            Self::Today => (day, add_days(day, 1)?),
            Self::Yesterday => (add_days(day, -1)?, day),
            Self::ThisWeek => {
                let monday = add_days(day, -i64::from(day.weekday().num_days_from_monday()))?;
                (monday, add_days(monday, 7)?)
            }
            Self::LastWeek => {
                let monday = add_days(day, -i64::from(day.weekday().num_days_from_monday()))?;
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
                NaiveDate::from_ymd_opt(day.year(), 1, 1)?,
                NaiveDate::from_ymd_opt(day.year() + 1, 1, 1)?,
            ),
            Self::LastYear => (
                NaiveDate::from_ymd_opt(day.year() - 1, 1, 1)?,
                NaiveDate::from_ymd_opt(day.year(), 1, 1)?,
            ),
        };
        Some(TemporalWindow {
            start: utc_text(offset, start),
            end: utc_text(offset, end),
        })
    }
}

fn add_days(date: NaiveDate, days: i64) -> Option<NaiveDate> {
    date.checked_add_signed(Duration::days(days))
}

fn month_start(year: i32, month: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, 1)
}

fn next_month(first: NaiveDate) -> Option<NaiveDate> {
    let (year, month) = (first.year(), first.month());
    if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
}

fn previous_month(first: NaiveDate) -> Option<NaiveDate> {
    let (year, month) = (first.year(), first.month());
    if month == 1 {
        NaiveDate::from_ymd_opt(year - 1, 12, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month - 1, 1)
    }
}

/// Midnight of `date` in `offset`, emitted as UTC `%Y-%m-%dT%H:%M:%SZ` text.
fn utc_text(offset: FixedOffset, date: NaiveDate) -> String {
    let local = offset
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .expect("a fixed offset maps each local time exactly once");
    local.naive_utc().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
#[path = "temporal_tests.rs"]
mod temporal_tests;

#[cfg(test)]
#[path = "temporal_arm_tests.rs"]
mod temporal_arm_tests;
