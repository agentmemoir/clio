// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the human text view's colour palette and the
//! plain-language match strength that sits beside a recall score.
//!
//! ## Owns
//! - The style palette and the one place colour is decided (`Palette`)
//! - Turning a calibrated stage score into a plain word (`match_strength`)
//!
//! ## Does not own
//! - The fused `final` score, which stays a raw rank-fusion value
//! - Output-mode selection or TTY detection (see `cli_output`)
//! - Which stage scores exist on a hit (see `cli_read_render`)
//!
//! ## Boundary
//!
//! Changes to how the text view looks or how match strength is worded should
//! normally require changes here. Changes to ranking or scoring should not.
//!
//! Keep this module focused on presentation of already-computed values.

use anstyle::{AnsiColor, Style};
use std::collections::BTreeSet;

/// Whether the text view paints its output.
///
/// `false` yields byte-identical plain text, so piped output, golden tests, and
/// `NO_COLOR` runs are unaffected by anything in this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Palette {
    enabled: bool,
}

impl Palette {
    /// A palette that paints, or one that does not when colour is unavailable.
    pub(crate) const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Paint `text` in `style`, or return it unchanged when colour is off.
    ///
    /// The escape sequence goes before the text and the reset goes after it.
    /// `Style::render_reset` cannot be used here: it emits prefix *and* reset as
    /// one unit, which would switch colour off before a single character is
    /// printed.
    pub(crate) fn paint(self, style: Style, text: &str) -> String {
        if self.enabled {
            format!("{}{text}{}", style.render(), anstyle::Reset)
        } else {
            text.to_owned()
        }
    }

    /// The hit's ordinal number, painted as a dim step marker.
    ///
    /// The trailing period is part of the word, not decoration: plain output
    /// must stay byte-identical to the uncoloured text this replaced.
    pub(crate) fn rank(self, index: usize) -> String {
        self.paint(
            Style::new().fg_color(Some(AnsiColor::BrightBlack.into())),
            &format!("{index}."),
        )
    }

    /// The `[semantic + lexical]` match-reason tag.
    pub(crate) fn reason(self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Cyan.into())), text)
    }

    /// The plain-language match strength, painted to read at a glance.
    pub(crate) fn strength(self, text: &str, strong: bool) -> String {
        let colour = if strong {
            AnsiColor::Green
        } else {
            AnsiColor::Yellow
        };
        self.paint(Style::new().fg_color(Some(colour.into())).bold(), text)
    }

    /// The `score 0.0240` fragment.
    pub(crate) fn score(self, text: &str) -> String {
        self.paint(
            Style::new().fg_color(Some(AnsiColor::BrightBlack.into())),
            text,
        )
    }

    /// The dim `id: … category: …` metadata line.
    pub(crate) fn meta(self, text: &str) -> String {
        self.paint(
            Style::new().fg_color(Some(AnsiColor::BrightBlack.into())),
            text,
        )
    }

    /// The `scores: final … semantic …` breakdown line.
    pub(crate) fn scores(self, text: &str) -> String {
        self.paint(Style::new().fg_color(Some(AnsiColor::Blue.into())), text)
    }

    /// The `Results for: …` heading.
    pub(crate) fn heading(self, text: &str) -> String {
        self.paint(Style::new().bold(), text)
    }
}

/// How closely a hit matched, in words rather than a fused rank score.
///
/// The fused `final` score is a rank-fusion total, so it is close to zero for a
/// perfect match and cannot be read as relevance. This word is derived from
/// signals that survive a change of embedding model:
///
/// - the dense arm's cosine similarity when it ran, and
/// - the lexical overlap of query terms when it did not.
///
/// Cosine bands are calibrated per embedding model, so they are only used when
/// the dense arm actually scored the hit, and the word always names the signal
/// it came from. A page that reached recall with neither signal gets no word at
/// all rather than a fabricated one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchStrength {
    /// Barely overlaps the query.
    Weak,
    /// Some overlap, not a close match.
    Fair,
    /// Closely matches the query.
    Strong,
}

impl MatchStrength {
    /// The word shown beside the hit.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Weak => "weak match",
            Self::Fair => "fair match",
            Self::Strong => "strong match",
        }
    }

    /// Whether the palette should paint this strength as reassuring.
    pub(crate) const fn is_strong(self) -> bool {
        matches!(self, Self::Strong)
    }

    /// The word, the signal it came from, and the number behind it.
    ///
    /// `semantic` is cosine similarity in `[-1, 1]`; `lexical` is the fraction
    /// of query terms the item's text contains. Returns `None` when neither arm
    /// produced a number, so the caller prints nothing.
    pub(crate) fn resolve(
        semantic: Option<f64>,
        lexical: Option<f64>,
    ) -> Option<(Self, &'static str, f64)> {
        if let Some(score) = semantic {
            return Some((Self::from_cosine(score), "cosine", score));
        }
        lexical.map(|score| (Self::from_overlap(score), "term overlap", score))
    }

    /// Band a cosine similarity.
    ///
    /// These two numbers are a property of the deployed embedding model, not of
    /// geometry, so they live here with that caveat rather than being presented
    /// as thresholds derived from first principles.
    fn from_cosine(score: f64) -> Self {
        if score >= 0.75 {
            Self::Strong
        } else if score >= 0.45 {
            Self::Fair
        } else {
            Self::Weak
        }
    }

    /// Band a fraction of query terms present in the item's text.
    ///
    /// Scale-free, so it survives an embedding-model change.
    fn from_overlap(fraction: f64) -> Self {
        if fraction >= 0.75 {
            Self::Strong
        } else if fraction >= 0.4 {
            Self::Fair
        } else {
            Self::Weak
        }
    }
}

/// The fraction of the query's terms that appear in `text`.
///
/// Returns `None` for an empty query, where the fraction would be undefined
/// rather than zero.
pub(crate) fn term_overlap(query: &str, text: &str) -> Option<f64> {
    let terms: BTreeSet<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if terms.is_empty() {
        return None;
    }
    let haystack = text.to_ascii_lowercase();
    let hits = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    // Term counts are bounded by the query's length, far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    Some(hits as f64 / terms.len() as f64)
}

#[cfg(test)]
#[path = "cli_style_tests.rs"]
mod cli_style_tests;
