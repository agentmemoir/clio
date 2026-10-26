// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for hygiene noise signal detection (
//! appendix pattern packs): secret-class hits and operational noise classes
//! detected over decrypted item text, plus the preview truncation cap.
//!
//! ## Owns
//! - [`detect_noise`]: pattern packs for secret / terminal spam / stack trace /
//!   heartbeat / command dump classes
//! - [`MAX_PREVIEW_CHARS`] and the truncated, secret-scrubbed preview builder
//!
//! ## Does not own
//! - Score composition or suggested-action bands (see `scoring`)
//! - Store scanning or pagination (see `audit`)
//!
//! ## Boundary
//!
//! Changes to detector heuristics should normally require changes here.
//! Changes to weights or ranking should not require changes here.
//!
//! Keep this module focused on noise signal detection.

use clio_config::secret::scrub_inline_secrets;

/// Preview cap: non-secret previews truncate to at most this many chars
pub const MAX_PREVIEW_CHARS: usize = 240;

/// One detected noise class with its evidence kind.
///
/// Reasons are stable ids shared with `noise_reasons[]` on audit candidates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoiseSignals {
    /// Real credential evidence found: `named_credential_key`, `pem_private_key`,
    /// `bearer_token`, `known_secret_format`.
    pub secret_kinds: Vec<&'static str>,
    /// Longest matched credential value length in bytes, `0` when none.
    pub secret_run_len: usize,
    /// Longest token-shaped run length in bytes; a weak proxy, never a hit.
    pub token_run_len: Option<usize>,
    /// Operational pattern kinds found (ids).
    pub pattern_kinds: Vec<&'static str>,
}

impl NoiseSignals {
    /// True when any secret-class hit was detected.
    #[must_use]
    pub fn secret_hit(&self) -> bool {
        !self.secret_kinds.is_empty()
    }

    /// Count of distinct operational pattern classes.
    #[must_use]
    pub fn pattern_distinct(&self) -> usize {
        self.pattern_kinds.len()
    }
}

/// Detect secret-class and operational-noise signals in item text.
///
/// A secret-class hit needs real credential evidence: a labeled key the shared
/// config masker scrubs, a PEM key block, a bearer token, or a known credential
/// format. Best-effort by design — pattern packs cannot guarantee coverage of
/// novel secret formats (documented limitation).
#[must_use]
pub fn detect_noise(text: &str) -> NoiseSignals {
    let mut signals = NoiseSignals::default();
    let lower = text.to_ascii_lowercase();

    // Secret-class: only real credential evidence counts. A long hyphenated
    // identifier mixes letters with digits too, so run shape alone is not one.
    if scrub_inline_secrets(text) != text {
        signals.secret_kinds.push("named_credential_key");
    }
    if lower.contains("-----begin")
        && (lower.contains("private key") || lower.contains("certificate"))
    {
        signals.secret_kinds.push("pem_private_key");
    }
    if lower.contains("bearer ") {
        let run = lower[rfind_bearer(&lower)..]
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .count();
        if run >= 24 {
            signals.secret_kinds.push("bearer_token");
            signals.secret_run_len = signals.secret_run_len.max(run);
        }
    }
    if let Some(len) = known_secret_format_len(text) {
        signals.secret_kinds.push("known_secret_format");
        signals.secret_run_len = signals.secret_run_len.max(len);
    }
    signals.token_run_len = token_shaped_run_len(&lower);

    // Terminal spam: ANSI escapes or prompt/exit chatter.
    if text.contains('\u{1b}')
        || count_lines_starting_with(text, '$') >= 3
        || count_occurrences(&lower, "command exited") >= 2
        || count_occurrences(&lower, "shell session") >= 2
    {
        signals.pattern_kinds.push("terminal_spam");
    }

    // Stack traces: multi-frame exception dumps without lesson structure.
    if lower.contains("traceback (most recent call last)")
        || lower.contains("panicked at")
        || count_lines_with_frame(text) >= 3
    {
        signals.pattern_kinds.push("stack_trace");
    }

    // Heartbeats: periodic ping / health chatter.
    if count_occurrences(&lower, "heartbeat") >= 3
        || count_occurrences(&lower, "health check") >= 3
        || count_occurrences(&lower, "ping ok") >= 3
        || count_occurrences(&lower, "pong") >= 3
    {
        signals.pattern_kinds.push("heartbeat");
    }

    // Command dumps: raw tool stdout without distilled lesson.
    if count_lines_starting_with(text, '$') >= 5 || count_occurrences(&lower, "exit code") >= 3 {
        signals.pattern_kinds.push("command_dump");
    }

    signals
}

/// Build a secret-safe preview: full `[REDACTED]` for secret-class text, else
/// inline-scrubbed and truncated to [`MAX_PREVIEW_CHARS`].
#[must_use]
pub fn masked_preview(text: &str, secret_class: bool) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    if secret_class {
        return Some("[REDACTED]".to_owned());
    }
    let scrubbed = scrub_inline_secrets(text);
    let short: String = scrubbed.chars().take(MAX_PREVIEW_CHARS).collect();
    Some(short)
}

/// End offset of the last `bearer ` marker (1-based style index used above).
fn rfind_bearer(lower: &str) -> usize {
    lower.rfind("bearer ").map_or(0, |i| i + "bearer ".len())
}

/// Longest token-shaped run: 24+ token chars mixing digits with letters.
///
/// This is a weak entropy proxy, not credential evidence, so callers must not
/// treat a hit as a secret.
fn token_shaped_run_len(lower: &str) -> Option<usize> {
    lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .filter(|word| word.len() >= 24)
        .filter(|word| {
            word.chars().any(|c: char| c.is_ascii_digit()) && word.chars().any(char::is_alphabetic)
        })
        .map(str::len)
        .max()
}

/// Known credential format prefixes paired with the body length each needs.
const KNOWN_SECRET_PREFIXES: &[(&str, usize)] = &[
    ("github_pat_", 20),
    ("sk_live_", 16),
    ("sk_test_", 16),
    ("rk_live_", 16),
    ("glpat-", 20),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("xoxa-", 10),
    ("xoxr-", 10),
    ("xoxs-", 10),
    ("AKIA", 16),
    ("ASIA", 16),
    ("AIza", 20),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("npm_", 20),
    ("sk-", 16),
];

/// Longest body length of a known credential format found in `text`.
///
/// A prefix only counts at a token boundary: the character before it is absent
/// or is neither ASCII alphanumeric nor `_`, so `risk-free` cannot match `sk-`.
fn known_secret_format_len(text: &str) -> Option<usize> {
    KNOWN_SECRET_PREFIXES
        .iter()
        .flat_map(|(prefix, min)| {
            text.match_indices(prefix).filter_map(move |(start, _)| {
                let boundary = text[..start]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
                if !boundary {
                    return None;
                }
                let body = text[start + prefix.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .count();
                (body >= *min).then_some(body)
            })
        })
        .max()
}

fn count_lines_starting_with(text: &str, marker: char) -> usize {
    text.lines()
        .filter(|line| line.trim_start().starts_with(marker))
        .count()
}

fn count_occurrences(lower: &str, needle: &str) -> usize {
    lower.matches(needle).count()
}

/// Lines that look like stack frames: ` at ` with a `file:line` shape.
fn count_lines_with_frame(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let Some(pos) = line.find(" at ") else {
                return false;
            };
            line[pos + 4..].contains(':')
        })
        .count()
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod detect_tests;
