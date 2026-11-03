// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for masking secret material in diagnostic output.
//!
//! ## Owns
//! - Secret path/key detection heuristics
//! - Last-4 / redacted string masking for scalars and JSON trees
//! - Char-boundary-safe inline `key: secret` scrubbing for prose telemetry
//!
//! ## Does not own
//! - Where secrets are stored or loaded
//! - Config merge order or tool handlers
//!
//! ## Boundary
//!
//! Changes to masking rules for config/log diagnostics should normally require changes here.
//! Changes to config schema fields should not require changes here unless new secret shapes appear.
//!
//! Keep this module focused on masking secrets in diagnostic output.

use serde_json::Value;

const SECRET_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "secret",
    "password",
    "token",
    "dek",
    "database_url",
    "credential",
    "auth",
    "private_key",
];

const INLINE_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "secret",
    "password",
    "token",
    "dek",
    "credential",
    "private_key",
];

/// True when a config path or field name should be treated as secret material.
///
/// Matching is token-based, not substring-based: a name is secret only when its
/// tokens contain a marker as a contiguous token sequence. `auth` matches
/// `user_auth` and `authToken`, but not `author`; `token` matches `auth_token`,
/// but not `tokens` or `lesson_max_tokens`.
#[must_use]
pub fn path_is_secret(path: &str) -> bool {
    let tokens = name_tokens(path);
    SECRET_MARKERS
        .iter()
        .any(|marker| contains_tokens(&tokens, &name_tokens(marker)))
}

/// Split a config path or field name into lowercase tokens.
///
/// Path separators (`.` and `[`) start a new segment, and inside a segment every
/// non-alphanumeric character and every camelCase boundary starts a new token.
fn name_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut after_lower_or_digit = false;
    for ch in name.chars() {
        if !ch.is_ascii_alphanumeric() {
            push_token(&mut tokens, &mut current);
            after_lower_or_digit = false;
            continue;
        }
        if ch.is_ascii_uppercase() && after_lower_or_digit {
            push_token(&mut tokens, &mut current);
        }
        after_lower_or_digit = !ch.is_ascii_uppercase();
        current.push(ch.to_ascii_lowercase());
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

/// True when `needle` appears in `haystack` as a contiguous run of tokens.
fn contains_tokens(haystack: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|run| run == needle)
}

/// Mask a secret string: empty or ≤4 chars → `[REDACTED]` (last-4 would reveal
/// the whole secret); otherwise `****` + last up to 4 chars.
///
/// [`REDACTED`] passes through unchanged: it is what the masker emits for a
/// short value, and masking an already-masked document must not rewrite it.
#[must_use]
pub fn mask_secret(raw: &str) -> String {
    if raw == REDACTED {
        return REDACTED.to_owned();
    }
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() <= 4 {
        return "[REDACTED]".to_owned();
    }
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("****{suffix}")
}

/// Recursively mask secret-looking **string** leaves in a JSON value (in place).
///
/// Keys are never renamed: a path read after masking still finds the field it
/// asked for. Only string leaves are rewritten, so a numeric, boolean or null
/// leaf keeps its exact value even when its name is a marker (a token count
/// under `lesson_max_tokens`, or `ranking.hygiene.secret` holding `0.25`).
pub fn mask_value(path: &str, value: &mut Value) {
    mask_string_secrets(path, value);
}

/// Return a masked deep clone of `value`.
#[must_use]
pub fn masked_clone(value: &Value) -> Value {
    let mut out = value.clone();
    mask_value("", &mut out);
    out
}

/// Mask a value that was selected out of a larger document at `path`.
///
/// A path lookup can land on a scalar (for example `credentials.api_key`),
/// which [`masked_clone`]'s object walk cannot reach; the path decides here
/// whether that scalar is masked before it leaves as a read view. A scalar that
/// is not a string is returned unchanged.
#[must_use]
pub(crate) fn masked_at_path(path: &str, value: &Value) -> Value {
    let mut out = value.clone();
    if out.is_object() || out.is_array() {
        mask_value(path, &mut out);
    } else if let Value::String(text) = &out
        && path_is_secret(path)
    {
        out = Value::String(mask_secret(text));
    }
    out
}

/// Recursively mask secret-looking **string** leaves in a JSON value (in place).
///
/// This is the walk [`mask_value`] delegates to. Callers that pass a domain
/// payload through a read view (history, audit) must not change its types: such
/// payloads carry non-secret numeric counters (`lesson_tokens`, `budget_tokens`),
/// and rewriting them would break the contract that the view returns the same
/// structured result as the tool it wraps.
///
/// A string leaf is masked when its key or full path looks secret; keys are
/// never renamed. Use [`scrub_inline_secrets`] to remove secrets embedded in
/// prose.
pub fn mask_string_secrets(path: &str, value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let secret = path_is_secret(&child_path) || path_is_secret(key);
                match child {
                    Value::Object(_) | Value::Array(_) => mask_string_secrets(&child_path, child),
                    Value::String(text) if secret => *text = mask_secret(text),
                    _ => {}
                }
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                mask_string_secrets(&format!("{path}[{i}]"), item);
            }
        }
        _ => {}
    }
}

/// The literal [`mask_secret`] emits for a value of four characters or fewer.
///
/// Masking is a fixed point on this literal: [`mask_secret`] returns it
/// unchanged, and [`scrub_inline_secrets`] short-circuits it. Without that, a
/// second masking pass over an already-masked document would rewrite the
/// ten-character `[REDACTED]` to `****TED]`. Longer masks already reproduce
/// themselves, because `****` plus the last four characters is itself.
const REDACTED: &str = "[REDACTED]";

/// Scrub every `key: secret` / `key=secret` pair in prose (all marker occurrences).
///
/// Idempotent: a value that is already [`REDACTED`] is left alone, so passing
/// an exported bundle back through the scrubber cannot mangle the masks the
/// export already applied. Indexes advance by whole characters so multi-byte
/// Unicode never splits a code point. Output is owned because masking replaces
/// substrings in place.
#[must_use]
pub fn scrub_inline_secrets(text: &str) -> String {
    let mut out = text.to_owned();
    for marker in INLINE_MARKERS {
        let mut search_from = 0;
        while let Some(start) = find_marker(&out, search_from, marker) {
            let after_marker = start + marker.len();
            let sep = out.get(after_marker..).and_then(|s| s.chars().next());
            if !matches!(sep, Some(':' | '=')) {
                search_from = after_marker;
                continue;
            }
            let value_start = skip_gaps(&out, after_marker + 1);
            let value_end = skip_value(&out, value_start);
            if value_end > value_start {
                let value = &out[value_start..value_end];
                if value == REDACTED {
                    // Already masked: leave it, and resume past it.
                    search_from = value_end;
                } else {
                    let masked = mask_secret(value);
                    out.replace_range(value_start..value_end, &masked);
                    search_from = value_start + masked.len();
                }
            } else {
                search_from = value_end;
            }
            if search_from >= out.len() {
                break;
            }
        }
    }
    out
}

fn find_marker(out: &str, from: usize, marker: &str) -> Option<usize> {
    debug_assert!(out.is_char_boundary(from));
    out.get(from..)
        .and_then(|hay| hay.to_ascii_lowercase().find(marker))
        .map(|rel| from + rel)
}

fn skip_gaps(out: &str, mut index: usize) -> usize {
    debug_assert!(out.is_char_boundary(index));
    while let Some(ch) = out.get(index..).and_then(|s| s.chars().next()) {
        if ch == ' ' || ch == '\t' {
            index += ch.len_utf8();
        } else {
            break;
        }
    }
    index
}

fn skip_value(out: &str, mut index: usize) -> usize {
    debug_assert!(out.is_char_boundary(index));
    while let Some(ch) = out.get(index..).and_then(|s| s.chars().next()) {
        if ch == ' ' || ch == '\t' || ch == '\n' {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

#[cfg(test)]
#[path = "secret_tests.rs"]
mod tests;
