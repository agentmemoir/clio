// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the CLI's "did you mean" matcher: deciding
//! that a word the caller typed is close enough to a real command or flag name to
//! offer, and which one.
//!
//! ## Owns
//! - The similarity measure, the cut-off, and the shared-first-letter rule
//! - `suggest`, the one entry point every caller uses
//!
//! ## Does not own
//! - The argument grammar that calls it (see `cli_args`)
//! - The command and flag names it is given (`cli_help` and each verb's spec)
//!
//! ## Boundary
//!
//! Changes to what counts as a typo should normally require changes here.
//! Adding a command or a flag should not.
//!
//! Keep this module focused on the matcher and nothing else. It is its own file
//! because the grammar is at its own size limit and this is a separate concern.

/// The similarity below which nothing is suggested, inclusive.
///
/// The value is the one `clap` uses for its own `did you mean` output, and the
/// boundary is included on purpose: `recod` against `recall` scores exactly
/// 0.7, and excluding the boundary would leave the most common kind of typo in
/// this CLI — a transposed or dropped letter in a short verb — with no
/// suggestion at all.
pub(crate) const SUGGEST_THRESHOLD: f64 = 0.7;

/// The closest candidate by Jaro similarity, when one is close enough.
///
/// Jaro rather than Levenshtein because it scores a one-letter slip in a short
/// word the way a reader does. `recod`/`recall` is three edits on six characters,
/// which an edit distance calls too far; Jaro calls it 0.7. It also keeps
/// `--limitt`/`--limit` high, because the two agree on five of six characters in
/// order.
///
/// Similarity alone is not enough, so a shared first letter is also required.
/// Jaro scores `frobnicate` against `consolidate` at 0.715 — six shared
/// characters in order is genuinely a lot — and offering `clio consolidate` for
/// `clio frobnicate` is worse than offering nothing. A mistyped word keeps its
/// first letter almost without exception, so that one cheap test removes the
/// whole family of suggestions that share letters without sharing a prefix.
///
/// Ties keep the earlier candidate, so the suggestion is a function of the
/// candidate order alone and never of how the comparison happened to land.
pub(crate) fn suggest<'a>(input: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let first = input.chars().next()?;
    let mut best: Option<(&'a str, f64)> = None;
    for &candidate in candidates {
        if !candidate.starts_with(first) {
            continue;
        }
        let score = jaro(input, candidate);
        if score < SUGGEST_THRESHOLD {
            continue;
        }
        if best.is_none_or(|(_, best_score)| score > best_score) {
            best = Some((candidate, score));
        }
    }
    best.map(|(candidate, _)| candidate)
}

/// `part` as a fraction of `whole`, for the two overlap ratios Jaro averages.
///
/// Every count this is called with counts characters of a command or flag name, so
/// it is tens at most and far below the 2^53 an `f64` holds exactly. The conversion
/// loses nothing, which is why it is allowed rather than worked around.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn ratio(part: usize, whole: usize) -> f64 {
    part as f64 / whole as f64
}

/// Jaro similarity of two strings, in `0.0..=1.0`.
///
/// The measure, and the reason for choosing it, are `clap`'s: it scores
/// `strsim::jaro` against a 0.7 cut-off. Written out here rather than pulled in
/// as a dependency, because this module already carries its own edit distance for
/// the same reason and the whole function is twenty lines.
///
/// Two characters match when they are equal and lie within
/// `max(len) / 2 - 1` positions of each other. The score is the mean of the two
/// character-overlap ratios and the ratio of matched characters that appear in
/// the same order, so a transposition costs half as much as a deletion.
pub(crate) fn jaro(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() || b.is_empty() {
        return f64::from(u8::from(a.is_empty() && b.is_empty()));
    }
    let window = (a.len().max(b.len()) / 2).saturating_sub(1);
    let mut a_matched = vec![false; a.len()];
    let mut b_matched = vec![false; b.len()];
    let mut matches = 0usize;
    for (i, &left) in a.iter().enumerate() {
        let low = i.saturating_sub(window);
        let high = (i + window + 1).min(b.len());
        for j in low..high {
            if !b_matched[j] && left == b[j] {
                a_matched[i] = true;
                b_matched[j] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    // Half the matched characters that appear in a different order, counted by
    // walking both match sequences forward together.
    let mut transpositions = 0usize;
    let mut j = 0usize;
    for (i, &left) in a.iter().enumerate() {
        if !a_matched[i] {
            continue;
        }
        while !b_matched[j] {
            j += 1;
        }
        if left != b[j] {
            transpositions += 1;
        }
        j += 1;
    }
    #[allow(clippy::cast_precision_loss)]
    let matches_f64 = matches as f64;
    // `transpositions / 2` is at most `matches / 2`, so the third term stays positive.
    #[allow(clippy::cast_precision_loss)]
    let in_order = matches_f64 - (transpositions / 2) as f64;
    (ratio(matches, a.len()) + ratio(matches, b.len()) + in_order / matches_f64) / 3.0
}

#[cfg(test)]
#[path = "cli_args_suggest_tests.rs"]
mod tests;
