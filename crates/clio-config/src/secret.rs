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

/// Markers that name a secret and nothing else, so even a number or boolean
/// below one is masked. `secret` and `token` are deliberately absent: they also
/// name counts and weights that a read view must return unchanged.
const STRONG_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "password",
    "private_key",
    "credential",
    "database_url",
    "auth",
    "dek",
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

/// True when a field name is unambiguous enough to mask a non-string leaf.
///
/// `secret` and `token` are left out on purpose: they name both secrets and
/// plain config (`ranking.hygiene.secret` is a weight, `lesson_max_tokens` is a
/// count), so a number under them stays a number. A name that contains any of
/// these markers as a whole token (`auth_pin`, `user_api_key`) is secret.
fn name_is_strong_secret(name: &str) -> bool {
    let tokens = name_tokens(name);
    STRONG_MARKERS
        .iter()
        .any(|marker| contains_tokens(&tokens, &name_tokens(marker)))
}

/// True when an object key is secret material rather than a field name.
///
/// A config field is a plain identifier (`api_key`, `author`, `credentials`,
/// `lesson_max_tokens`), and those keep their names so a path read still finds
/// them. A key that is not an identifier at all (`sk-live-SECRETKEY99`) can only
/// be data, so a marker anywhere in it means the key itself is the secret.
fn key_is_secret_data(key: &str) -> bool {
    !is_identifier(key) && contains_marker(key)
}

/// True for a plain field name: a letter or `_`, then letters, digits or `_`.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => {
            (first.is_ascii_alphabetic() || first == '_')
                && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        }
        None => false,
    }
}

/// True when any marker appears in `name` as a case-insensitive substring.
fn contains_marker(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|marker| lowered.contains(marker))
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

/// Recursively mask secret-looking leaves in a JSON value (in place).
///
/// Field names are kept so a path read after masking still finds the field it
/// asked for. Only a key that is not an identifier — secret material stored as a
/// key name — is replaced. Leaves under an unambiguous secret name are masked
/// whatever their JSON type; elsewhere only strings are rewritten, so a count
/// keeps its number.
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
/// whether that scalar is masked before it leaves as a read view.
#[must_use]
pub(crate) fn masked_at_path(path: &str, value: &Value) -> Value {
    let mut out = value.clone();
    if out.is_object() || out.is_array() {
        mask_value(path, &mut out);
    } else {
        let key = path.rsplit('.').next().unwrap_or(path);
        mask_member(path, key, &mut out);
    }
    out
}

/// Recursively mask secret-looking leaves in a JSON value (in place).
///
/// This is the walk [`mask_value`] delegates to. Callers that pass a domain
/// payload through a read view (history, audit) must not lose its shape: field
/// names stay, counters stay numbers. Only a key that cannot be a field name —
/// secret material stored as a key, `sk-live-SECRETKEY99` — is replaced.
///
/// A string leaf is masked when its key or full path looks secret. A number or
/// boolean is masked only under an unambiguous secret name (`password`,
/// `api_key`, `auth_pin`), so counts such as `lesson_max_tokens` and the
/// `ranking.hygiene.secret` weight keep their value. Use [`scrub_inline_secrets`]
/// to remove secrets embedded in prose.
pub fn mask_string_secrets(path: &str, value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.keys().any(|key| key_is_secret_data(key)) {
                mask_data_keys(path, map);
                return;
            }
            for (key, child) in map.iter_mut() {
                mask_member(&join_path(path, key), key, child);
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

/// Mask one object member in place: containers are walked, leaves are masked
/// when their name or path makes them secret.
fn mask_member(child_path: &str, key: &str, child: &mut Value) {
    match child {
        Value::Object(_) | Value::Array(_) => mask_string_secrets(child_path, child),
        Value::String(text)
            if key_is_secret_data(key) || path_is_secret(child_path) || path_is_secret(key) =>
        {
            *text = mask_secret(text);
        }
        // A masked number is the whole literal: a last-4 form would publish the
        // digits of a PIN. Counts never reach this arm, see `name_is_strong_secret`.
        Value::Number(_) | Value::Bool(_) if name_is_strong_secret(key) => {
            *child = Value::String(REDACTED.to_owned());
        }
        _ => {}
    }
}

/// Rebuild `map`, replacing each key that is secret data with its mask.
///
/// Every entry is kept: a masked key that another entry already owns gets a
/// numbered suffix instead of overwriting it.
fn mask_data_keys(path: &str, map: &mut serde_json::Map<String, Value>) {
    let entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
    let mut rebuilt = serde_json::Map::with_capacity(entries.len());
    for (key, mut child) in entries {
        mask_member(&join_path(path, &key), &key, &mut child);
        let name = if key_is_secret_data(&key) {
            unique_key(REDACTED.to_owned(), &rebuilt)
        } else {
            key
        };
        rebuilt.insert(name, child);
    }
    *map = rebuilt;
}

fn join_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// A candidate key no earlier entry owns; suffixed when a masked form collides.
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
