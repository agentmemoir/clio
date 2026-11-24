// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for locatable date/time span parse matching.
//!
//! ## Owns
//! - Format preference list, ambiguous numeric fail-closed, precision compare
//! - Char-boundary-safe month-name token windows (no UTF-8 mid-codepoint slices)
//! - Lightweight multilingual month alias normalization (de/fr/es → English)
//!
//! ## Does not own
//! - Entity/number matchers or verify orchestration
//!
//! ## Boundary
//!
//! Changes to date format policy should normally require changes here.

use crate::verify::VerifyConfig;
use jiff::civil::{Date, DateTime};
use serde_json::Value;

/// Unambiguous format strings tried before numeric slash dates.
const SAFE_DATE: &[&str] = &["%Y-%m-%d", "%d %b %Y", "%b %d, %Y", "%d %B %Y", "%B %d, %Y"];
const SAFE_DATETIME: &[&str] = &["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"];

/// Longest original month word a span window keeps.
const MONTH_WORD_MAX: usize = 19;
/// Longest non-alphabetic run kept after the month word in a span window.
const MONTH_TAIL_MAX: usize = 24;

/// Parsed instant at the precision present in the snapshot or source span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedInstant {
    Date(Date),
    DateTime(DateTime),
}

impl ParsedInstant {
    fn matches(self, other: Self) -> bool {
        match (self, other) {
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::DateTime(a), Self::DateTime(b)) => a == b,
            // Snapshot date-only may ground against a source date-time on that day.
            (Self::Date(a), Self::DateTime(b)) => a == b.date(),
            // Snapshot with time requires a matching time in source (no hallucination).
            (Self::DateTime(_), Self::Date(_)) => false,
        }
    }
}

/// Locate a source substring that parses to the same calendar instant as `value`.
/// Returns character (Unicode scalar) offsets into `source`, not byte offsets, per `SpanOffsets`.
///
/// # Errors
/// Unparsable snapshot date, ambiguous numeric date without `day_first`, or no span.
pub fn match_date(
    source: &str,
    value: &Value,
    config: &VerifyConfig,
) -> Result<(usize, usize), String> {
    let expected = snapshot_instant(value)?;
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut saw_ambiguous_match = false;
    while i < chars.len() {
        if let Some((end, token)) = next_date_token(&chars, i) {
            match try_token(&token, expected, config) {
                TokenOutcome::Match => return Ok((i, end)),
                TokenOutcome::AmbiguousWouldMatch => saw_ambiguous_match = true,
                TokenOutcome::No => {}
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if saw_ambiguous_match {
        return Err("ambiguous date without day_first policy".into());
    }
    Err("no locatable date span parses to snapshot value".into())
}

enum TokenOutcome {
    Match,
    AmbiguousWouldMatch,
    No,
}

fn try_token(token: &str, expected: ParsedInstant, config: &VerifyConfig) -> TokenOutcome {
    match parse_date_token(token, config) {
        Ok(Some(got)) if expected.matches(got) => TokenOutcome::Match,
        Err(reason) if reason == "ambiguous" => {
            let Some((a, b, y)) = split_numeric_date(token) else {
                return TokenOutcome::No;
            };
            let day_first = ymd_option(y, b, a).map(ParsedInstant::Date);
            let month_first = ymd_option(y, a, b).map(ParsedInstant::Date);
            if day_first.is_some_and(|d| expected.matches(d))
                || month_first.is_some_and(|d| expected.matches(d))
            {
                TokenOutcome::AmbiguousWouldMatch
            } else {
                TokenOutcome::No
            }
        }
        Ok(_) | Err(_) => TokenOutcome::No,
    }
}

fn snapshot_instant(value: &Value) -> Result<ParsedInstant, String> {
    let s = value
        .as_str()
        .ok_or_else(|| "date field must be a string".to_owned())?
        .trim();
    if s.is_empty() {
        return Err("empty date value".into());
    }
    if let Ok(dt) = DateTime::strptime("%Y-%m-%dT%H:%M:%S", s) {
        return Ok(ParsedInstant::DateTime(dt));
    }
    if let Ok(dt) = DateTime::strptime("%Y-%m-%d %H:%M:%S", s) {
        return Ok(ParsedInstant::DateTime(dt));
    }
    if let Ok(d) = Date::strptime("%Y-%m-%d", s) {
        return Ok(ParsedInstant::Date(d));
    }
    Err("snapshot date must be ISO date or date-time".into())
}

fn next_date_token(chars: &[char], start: usize) -> Option<(usize, String)> {
    if chars[start].is_ascii_digit() {
        let mut end = start;
        while end < chars.len() {
            let ch = chars[end];
            if ch.is_ascii_digit() || matches!(ch, '-' | '/' | '.' | ':' | 'T') {
                end += 1;
            } else {
                break;
            }
        }
        if end > start + 4 {
            let token: String = chars[start..end].iter().collect();
            return Some((end, token));
        }
        return None;
    }
    if chars[start].is_alphabetic() {
        return month_name_token(chars, start);
    }
    None
}

/// Build a month-name window using char counts (never byte-index into UTF-8).
fn month_name_token(chars: &[char], start: usize) -> Option<(usize, String)> {
    let run = &chars[start..];
    let head = &run[..run.len().min(MONTH_WORD_MAX)];
    let word_len = head.iter().take_while(|c| c.is_alphabetic()).count();
    let tail = run[word_len..].iter().take(MONTH_TAIL_MAX);
    let end = start + word_len + tail.take_while(|c| !c.is_alphabetic()).count();
    for fmt in ["%b %d, %Y", "%B %d, %Y", "%d %b %Y", "%d %B %Y"] {
        for take in (6..=end - start).rev() {
            let orig: String = chars[start..start + take].iter().collect();
            let orig = orig.trim_end();
            if Date::strptime(fmt, localize_months(orig)).is_ok() {
                return Some((start + take, orig.to_owned()));
            }
        }
    }
    None
}

/// Map common non-English month tokens to English abbreviations jiff accepts.
fn localize_months(s: &str) -> String {
    const ALIASES: &[(&str, &str)] = &[
        ("März", "Mar"),
        ("août", "Aug"),
        ("Août", "Aug"),
        ("dezember", "Dec"),
        ("Dezember", "Dec"),
        ("décembre", "Dec"),
        ("Décembre", "Dec"),
        ("septiembre", "Sep"),
        ("Septiembre", "Sep"),
    ];
    let mut out = s.to_owned();
    for (from, to) in ALIASES {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

fn parse_date_token(token: &str, config: &VerifyConfig) -> Result<Option<ParsedInstant>, String> {
    let token = localize_months(token);
    for fmt in SAFE_DATETIME {
        if let Ok(dt) = DateTime::strptime(fmt, &token) {
            return Ok(Some(ParsedInstant::DateTime(dt)));
        }
    }
    for fmt in SAFE_DATE {
        if let Ok(d) = Date::strptime(fmt, &token) {
            return Ok(Some(ParsedInstant::Date(d)));
        }
    }
    if let Some((a, b, y)) = split_numeric_date(&token) {
        if a > 12 && b <= 12 {
            return Ok(ymd_option(y, b, a).map(ParsedInstant::Date));
        }
        if b > 12 && a <= 12 {
            return Ok(ymd_option(y, a, b).map(ParsedInstant::Date));
        }
        if a <= 12 && b <= 12 {
            return match config.day_first {
                None => Err("ambiguous".into()),
                Some(true) => Ok(ymd_option(y, b, a).map(ParsedInstant::Date)),
                Some(false) => Ok(ymd_option(y, a, b).map(ParsedInstant::Date)),
            };
        }
    }
    Ok(None)
}

/// Build a civil date from numeric parts, failing closed on out-of-range parts.
fn ymd_option(year: i32, month: u32, day: u32) -> Option<Date> {
    let (Ok(year), Ok(month), Ok(day)) =
        (i16::try_from(year), i8::try_from(month), i8::try_from(day))
    else {
        return None;
    };
    Date::new(year, month, day).ok()
}

fn split_numeric_date(token: &str) -> Option<(u32, u32, i32)> {
    let sep = if token.contains('/') {
        '/'
    } else if token.matches('.').count() >= 2 && !token.contains('-') {
        '.'
    } else {
        return None;
    };
    let parts: Vec<&str> = token.split(sep).collect();
    if parts.len() != 3 {
        return None;
    }
    let a: u32 = parts[0].parse().ok()?;
    let b: u32 = parts[1].parse().ok()?;
    let y: i32 = parts[2].parse().ok()?;
    if y < 100 {
        return None;
    }
    Some((a, b, y))
}

#[cfg(test)]
#[path = "verify_date_tests.rs"]
mod verify_date_tests;
