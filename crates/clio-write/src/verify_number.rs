// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for locatable number-span parse matching.
//!
//! ## Owns
//! - Digit-run discovery, locale grouping/decimal normalize, tolerance compare
//! - Token boundaries that do not merge range operators (`10 - 15`)
//!
//! ## Does not own
//! - Entity/date matchers or verify orchestration
//!
//! ## Boundary
//!
//! Changes to number parse/locale rules should normally require changes here.

use crate::verify::VerifyConfig;
use serde_json::Value;

const CURRENCY: &[char] = &['$', '€', '£', '¥'];

/// Locate a source substring that parses to the same number as `value`.
///
/// # Errors
/// Unparsable snapshot value, or no locatable parseable span.
pub fn match_number(
    source: &str,
    value: &Value,
    config: &VerifyConfig,
) -> Result<(usize, usize), String> {
    let expected = snapshot_number(value)?;
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if is_num_start(chars[i]) {
            let (end, parsed) = scan_number(&chars, i, config);
            if let Some(n) = parsed
                && numbers_equal(n, expected, config.number_tolerance)
            {
                return Ok((i, end));
            }
            // Advance one char on miss so ranges like `10 - 15` still find both.
            i += 1;
        } else {
            i += 1;
        }
    }
    Err("no locatable number span parses to snapshot value".into())
}

fn snapshot_number(value: &Value) -> Result<f64, String> {
    if let Some(n) = value.as_f64() {
        return Ok(n);
    }
    if let Some(s) = value.as_str() {
        return parse_number_token(s.trim(), true)
            .ok_or_else(|| "number field string did not parse".to_owned());
    }
    Err("number field must be a number or numeric string".into())
}

fn is_num_start(ch: char) -> bool {
    ch.is_ascii_digit() || CURRENCY.contains(&ch) || ch == '+' || ch == '-'
}

/// Scan one numeric token: optional leading sign/currency, then digits with
/// one locale decimal and grouping separators. Does **not** consume spaces or
/// additional `+`/`-` operators (those start a new scan from the next char).
fn scan_number(chars: &[char], start: usize, config: &VerifyConfig) -> (usize, Option<f64>) {
    let mut end = start;
    // Leading currency and at most one sign.
    while end < chars.len() && CURRENCY.contains(&chars[end]) {
        end += 1;
    }
    if end < chars.len() && (chars[end] == '+' || chars[end] == '-') {
        end += 1;
    }
    while end < chars.len() && CURRENCY.contains(&chars[end]) {
        end += 1;
    }
    let digit_start = end;
    while end < chars.len() {
        let ch = chars[end];
        if ch.is_ascii_digit() || ch == ',' || ch == '.' {
            end += 1;
        } else {
            break;
        }
    }
    if end == digit_start {
        return (start + 1, None);
    }
    let token: String = chars[start..end].iter().collect();
    let parsed = parse_number_token(token.trim(), config.comma_grouping);
    (end, parsed)
}

fn parse_number_token(raw: &str, comma_grouping: bool) -> Option<f64> {
    let mut s = raw.to_owned();
    for c in CURRENCY {
        s = s.replace(*c, "");
    }
    s = s.replace(' ', "");
    if s.is_empty() {
        return None;
    }
    let normalized = if comma_grouping {
        s.replace(',', "")
    } else {
        let tmp = s.replace('.', "");
        tmp.replace(',', ".")
    };
    normalized.parse::<f64>().ok()
}

fn numbers_equal(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}
