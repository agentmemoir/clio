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
    let end = (start + 18).min(chars.len());
    let window: String = chars[start..end].iter().collect();
    let normalized = localize_months(&window);
    for fmt in ["%b %d, %Y", "%B %d, %Y", "%d %b %Y", "%d %B %Y"] {
        let char_len = normalized.chars().count();
        for take in (6..=char_len).rev() {
            let candidate: String = normalized.chars().take(take).collect();
            let candidate = candidate.trim_end();
            if Date::strptime(fmt, candidate).is_err() {
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
mod tests {
    use super::*;
    use crate::verify::VerifyConfig;

    #[test]
    fn month_aliases_normalize_to_parsable_english() {
        const CASES: &[(&str, &str, &str)] = &[
            ("März", "Mar", "2026-03-03"),
            ("août", "Aug", "2026-08-03"),
            ("Août", "Aug", "2026-08-03"),
            ("dezember", "Dec", "2026-12-03"),
            ("Dezember", "Dec", "2026-12-03"),
            ("décembre", "Dec", "2026-12-03"),
            ("Décembre", "Dec", "2026-12-03"),
            ("septiembre", "Sep", "2026-09-03"),
            ("Septiembre", "Sep", "2026-09-03"),
        ];
        for (alias, english, expected) in CASES {
            let normalized = localize_months(&format!("{alias} 3, 2026"));
            assert_eq!(normalized, format!("{english} 3, 2026"), "alias {alias}");
            let parsed = Date::strptime("%b %d, %Y", &normalized)
                .unwrap_or_else(|err| panic!("alias {alias} failed: {err}"));
            assert_eq!(parsed.to_string(), *expected, "alias {alias}");
        }
    }

    #[test]
    fn ambiguous_numeric_date_fails_closed() {
        let value = serde_json::json!("2026-03-04");
        let err = match_date("on 03/04/2026", &value, &VerifyConfig::default())
            .expect_err("ambiguous numeric date must fail closed");
        assert_eq!(err, "ambiguous date without day_first policy");
    }

    #[test]
    fn iso_datetime_in_source_matches_snapshot() {
        let value = serde_json::json!("2026-03-04T05:06:07");
        let source = "at 2026-03-04T05:06:07 UTC";
        let (start, end) = match_date(source, &value, &VerifyConfig::default())
            .expect("ISO date-time span must match");
        assert_eq!(&source[start..end], "2026-03-04T05:06:07");
    }

    #[test]
    fn snapshot_date_matches_source_datetime_on_same_day() {
        let value = serde_json::json!("2026-03-04");
        let source = "on 2026-03-04T05:06:07";
        let (start, end) = match_date(source, &value, &VerifyConfig::default())
            .expect("date-only snapshot must match a date-time span on that day");
        assert_eq!(&source[start..end], "2026-03-04T05:06:07");
    }

    #[test]
    fn snapshot_datetime_does_not_match_date_only_span() {
        let value = serde_json::json!("2026-03-04T05:06:07");
        let err = match_date("on 2026-03-04", &value, &VerifyConfig::default())
            .expect_err("a date-only span must not satisfy a snapshot with a time");
        assert_eq!(err, "no locatable date span parses to snapshot value");
    }

    #[test]
    fn month_name_in_source_matches_snapshot() {
        let value = serde_json::json!("2026-03-03");
        let source = "on March 3, 2026";
        let (start, end) = match_date(source, &value, &VerifyConfig::default())
            .expect("month-name span must match");
        assert_eq!(&source[start..end], "March 3, 2026");
    }

    #[test]
    fn snapshot_accepts_space_separated_datetime() {
        let parsed = snapshot_instant(&serde_json::json!("2026-03-04 05:06:07"))
            .expect("space-separated ISO date-time must parse");
        let expected = DateTime::strptime("%Y-%m-%dT%H:%M:%S", "2026-03-04T05:06:07")
            .expect("the equivalent ISO date-time must parse");
        assert_eq!(parsed, ParsedInstant::DateTime(expected));
    }

    #[test]
    fn snapshot_rejects_empty_and_non_iso_values() {
        assert_eq!(
            snapshot_instant(&serde_json::json!("")),
            Err("empty date value".to_owned())
        );
        assert_eq!(
            snapshot_instant(&serde_json::json!("yesterday")),
            Err("snapshot date must be ISO date or date-time".to_owned())
        );
    }

    #[test]
    fn numeric_date_respects_day_first_policy() {
        let value = serde_json::json!("2026-03-04");
        let source = "on 03/04/2026";
        let month_first = VerifyConfig {
            day_first: Some(false),
            ..Default::default()
        };
        let (start, end) = match_date(source, &value, &month_first)
            .expect("month-first policy must match 03/04/2026");
        assert_eq!(&source[start..end], "03/04/2026");
        let day_first = VerifyConfig {
            day_first: Some(true),
            ..Default::default()
        };
        assert_eq!(
            match_date(source, &value, &day_first).err(),
            Some("no locatable date span parses to snapshot value".to_owned())
        );
    }

    #[test]
    fn unambiguous_numeric_dates_match_without_policy() {
        let value = serde_json::json!("2026-03-25");
        for (source, span) in [
            ("on 25/03/2026", "25/03/2026"),
            ("on 03/25/2026", "03/25/2026"),
        ] {
            let (start, end) = match_date(source, &value, &VerifyConfig::default())
                .expect("an unambiguous numeric date must match without day_first");
            assert_eq!(&source[start..end], span);
        }
    }

    #[test]
    fn ambiguous_span_that_cannot_match_does_not_locate_a_span() {
        let value = serde_json::json!("2027-01-01");
        let err = match_date("on 03/04/2026", &value, &VerifyConfig::default())
            .expect_err("an ambiguous span that cannot match must not locate a span");
        assert_eq!(err, "no locatable date span parses to snapshot value");
    }

    #[test]
    fn out_of_range_numeric_span_does_not_locate_a_match() {
        let value = serde_json::json!("2026-03-04");
        let err = match_date("on 99/99/2026", &value, &VerifyConfig::default())
            .expect_err("out-of-range numeric parts must not locate a span");
        assert_eq!(err, "no locatable date span parses to snapshot value");
    }

    #[test]
    fn ymd_option_fails_closed_on_out_of_range_parts() {
        assert!(ymd_option(40000, 3, 4).is_none());
        assert!(ymd_option(2026, 200, 4).is_none());
        assert!(ymd_option(2026, 3, 200).is_none());
        assert_eq!(
            ymd_option(2026, 3, 4),
            Some(Date::new(2026, 3, 4).expect("valid date"))
        );
    }

    #[test]
    fn split_numeric_date_handles_dot_and_rejects_invalid_shapes() {
        assert_eq!(split_numeric_date("03.04.2026"), Some((3, 4, 2026)));
        assert!(split_numeric_date("2026-03-04").is_none());
        assert!(split_numeric_date("03/04").is_none());
        assert!(split_numeric_date("03/04/99").is_none());
    }
}
