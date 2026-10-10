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
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::Value;

/// Unambiguous format strings tried before numeric slash dates.
const SAFE_DATE: &[&str] = &["%Y-%m-%d", "%d %b %Y", "%b %d, %Y", "%d %B %Y", "%B %d, %Y"];
const SAFE_DATETIME: &[&str] = &["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"];

/// Parsed instant at the precision present in the snapshot or source span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedInstant {
    Date(NaiveDate),
    DateTime(NaiveDateTime),
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
            let day_first = NaiveDate::from_ymd_opt(y, b, a).map(ParsedInstant::Date);
            let month_first = NaiveDate::from_ymd_opt(y, a, b).map(ParsedInstant::Date);
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
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(ParsedInstant::DateTime(dt));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(ParsedInstant::DateTime(dt));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
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
    let end = (start + 18).min(chars.len());
    let window: String = chars[start..end].iter().collect();
    let normalized = localize_months(&window);
    for fmt in ["%b %d, %Y", "%B %d, %Y", "%d %b %Y", "%d %B %Y"] {
        let char_len = normalized.chars().count();
        for take in (6..=char_len).rev() {
            let candidate: String = normalized.chars().take(take).collect();
            let candidate = candidate.trim_end();
            if NaiveDate::parse_from_str(candidate, fmt).is_err() {
                continue;
            }
            // Map localized prefix length back onto the original UTF-8 window.
            let max_orig = (end - start).min(take + 6);
            for orig_take in take..=max_orig {
                let orig: String = chars[start..start + orig_take].iter().collect();
                let loc = localize_months(orig.trim_end());
                if loc == candidate {
                    return Some((start + orig_take, orig.trim_end().to_owned()));
                }
            }
        }
    }
    None
}

/// Map common non-English month tokens to English abbreviations chrono accepts.
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
        if let Ok(dt) = NaiveDateTime::parse_from_str(&token, fmt) {
            return Ok(Some(ParsedInstant::DateTime(dt)));
        }
    }
    for fmt in SAFE_DATE {
        if let Ok(d) = NaiveDate::parse_from_str(&token, fmt) {
            return Ok(Some(ParsedInstant::Date(d)));
        }
    }
    if let Some((a, b, y)) = split_numeric_date(&token) {
        if a > 12 && b <= 12 {
            return Ok(NaiveDate::from_ymd_opt(y, b, a).map(ParsedInstant::Date));
        }
        if b > 12 && a <= 12 {
            return Ok(NaiveDate::from_ymd_opt(y, a, b).map(ParsedInstant::Date));
        }
        if a <= 12 && b <= 12 {
            return match config.day_first {
                None => Err("ambiguous".into()),
                Some(true) => Ok(NaiveDate::from_ymd_opt(y, b, a).map(ParsedInstant::Date)),
                Some(false) => Ok(NaiveDate::from_ymd_opt(y, a, b).map(ParsedInstant::Date)),
            };
        }
    }
    Ok(None)
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
