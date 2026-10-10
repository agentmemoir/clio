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
#[must_use]
pub fn path_is_secret(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|m| lower.contains(m))
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

/// Recursively mask secret-looking keys in a JSON value (in place).
///
/// A key that itself looks secret is replaced by its masked form, so secret
/// material stored in a key name cannot survive the walk. Values keep the
/// rules they had: leaves under a secret path become masked strings, and
/// containers are walked.
pub fn mask_value(path: &str, value: &mut Value) {
    match value {
        Value::Object(map) => {
            let entries = std::mem::take(map);
            for (key, mut child) in entries {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let secret = path_is_secret(&child_path) || path_is_secret(&key);
                if child.is_object() || child.is_array() {
                    mask_value(&child_path, &mut child);
                } else if secret {
                    child = Value::String(mask_leaf(&child));
                }
                let out_key = if path_is_secret(&key) {
                    unique_key(mask_secret(&key), map)
                } else {
                    unique_key(key, map)
                };
                map.insert(out_key, child);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter_mut().enumerate() {
                mask_value(&format!("{path}[{i}]"), item);
            }
        }
        _ => {}
    }
}

/// A key no earlier entry owns; suffixed when a masked form collides.
///
/// Two secret-looking siblings can mask to the same last-4 form (`embed_api_key`
/// and `rerank_api_key` both become `****_key`). Dropping the later one would
/// silently delete an entry, so it gets a numbered suffix instead.
fn unique_key(candidate: String, taken: &serde_json::Map<String, Value>) -> String {
    if !taken.contains_key(&candidate) {
        return candidate;
    }
    let mut n = 2;
    loop {
        let attempt = format!("{candidate}#{n}");
        if !taken.contains_key(&attempt) {
            return attempt;
        }
        n += 1;
    }
}

fn mask_leaf(value: &Value) -> String {
    match value {
        Value::String(s) => mask_secret(s),
        Value::Null => "[REDACTED]".to_owned(),
        other => mask_secret(&other.to_string()),
    }
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
/// whether that scalar is masked before it leaves as a read view.
#[must_use]
pub(crate) fn masked_at_path(path: &str, value: &Value) -> Value {
    let mut out = value.clone();
    if out.is_object() || out.is_array() {
        mask_value(path, &mut out);
    } else if path_is_secret(path) {
        out = Value::String(mask_leaf(value));
    }
    out
}

/// Recursively mask secret-looking **string** leaves in a JSON value (in place).
///
/// This is [`mask_value`] restricted to string leaves. Callers that pass a
/// domain payload through a read view (history, audit) must not change its
/// types: such payloads carry non-secret numeric counters whose names merely
/// contain a marker substring (for example `lesson_tokens`), and rewriting them
/// to `"[REDACTED]"` would break the contract that the view returns the same
/// structured result as the tool it wraps.
///
/// A string leaf is masked when its key or full path looks secret. Use
/// [`scrub_inline_secrets`] to remove secrets embedded in prose.
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
