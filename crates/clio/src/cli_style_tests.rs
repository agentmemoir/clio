// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Unit tests for the text view's palette and match-strength wording.

use super::*;

/// Drop CSI escape sequences so painted text can be compared with plain text.
fn strip_escapes(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn a_plain_palette_emits_no_escape_codes() {
    let palette = Palette::new(false);
    for painted in [
        palette.rank(1),
        palette.reason("semantic + lexical"),
        palette.strength("strong match", true),
        palette.score("score 0.0240"),
        palette.meta("id: itm-1"),
        palette.scores("scores: final 0.0240"),
        palette.heading("Results for: q"),
    ] {
        assert!(!painted.contains('\u{1b}'), "{painted:?}");
    }
}

#[test]
fn a_plain_palette_is_byte_identical_to_the_input() {
    let palette = Palette::new(false);
    assert_eq!(palette.heading("Results for: q"), "Results for: q");
    assert_eq!(palette.rank(3), "3.");
    assert_eq!(palette.strength("weak match", false), "weak match");
}

#[test]
fn an_enabled_palette_wraps_the_text_in_a_reset() {
    let painted = Palette::new(true).heading("Results for: q");
    assert!(painted.starts_with("\u{1b}["), "{painted:?}");
    // The escape opens the colour and the reset closes it *after* the words, so
    // colour cannot bleed into the next line and cannot switch off early.
    assert!(
        painted.find("Results for: q").unwrap() < painted.rfind('\u{1b}').unwrap(),
        "the reset must come after the text: {painted:?}"
    );
    assert!(painted.ends_with("\u{1b}[0m"), "{painted:?}");
}

#[test]
fn the_two_palettes_carry_the_same_words() {
    assert_eq!(strip_escapes(&Palette::new(true).rank(2)), "2.");
    assert_eq!(Palette::new(false).rank(2), "2.");
}

#[test]
fn the_dense_signal_is_preferred_and_named() {
    let (strength, signal, score) =
        MatchStrength::resolve(Some(0.8371), Some(0.1)).expect("staged");
    assert_eq!(strength, MatchStrength::Strong);
    assert_eq!(signal, "cosine");
    assert!((score - 0.8371).abs() < 1e-9);
}

#[test]
fn a_lexical_only_page_still_gets_a_word() {
    // The reader who most needs the guidance is the one whose dense arm never
    // ran, so the fallback must exist rather than leaving the line off.
    let (strength, signal, score) = MatchStrength::resolve(None, Some(0.75)).expect("staged");
    assert_eq!(strength, MatchStrength::Strong);
    assert_eq!(signal, "term overlap");
    assert!((score - 0.75).abs() < 1e-9);
}

#[test]
fn no_signal_means_no_word() {
    assert!(MatchStrength::resolve(None, None).is_none());
}

#[test]
fn term_overlap_counts_present_query_terms() {
    let text = "the user prefers dark mode";
    assert!((term_overlap("user prefers", text).unwrap() - 1.0).abs() < 1e-9);
    assert!((term_overlap("user zebra", text).unwrap() - 0.5).abs() < 1e-9);
    assert!((term_overlap("zebra quail", text).unwrap() - 0.0).abs() < 1e-9);
    // Case must not change the count.
    assert!((term_overlap("USER PREFERS", text).unwrap() - 1.0).abs() < 1e-9);
}

#[test]
fn an_empty_query_has_no_overlap_rather_than_zero() {
    assert!(term_overlap("", "some text").is_none());
    assert!(term_overlap("   ", "some text").is_none());
}

#[test]
fn overlap_bands_cover_the_range() {
    let band = |f| MatchStrength::resolve(None, Some(f)).map(|(s, _, _)| s);
    assert_eq!(band(1.0), Some(MatchStrength::Strong));
    assert_eq!(band(0.75), Some(MatchStrength::Strong));
    assert_eq!(band(0.74), Some(MatchStrength::Fair));
    assert_eq!(band(0.4), Some(MatchStrength::Fair));
    assert_eq!(band(0.39), Some(MatchStrength::Weak));
    assert_eq!(band(0.0), Some(MatchStrength::Weak));
}

#[test]
fn strength_words_are_distinct_and_stable() {
    assert_eq!(MatchStrength::Weak.as_str(), "weak match");
    assert_eq!(MatchStrength::Fair.as_str(), "fair match");
    assert_eq!(MatchStrength::Strong.as_str(), "strong match");
    assert!(MatchStrength::Strong.is_strong());
    assert!(!MatchStrength::Weak.is_strong());
    assert!(!MatchStrength::Fair.is_strong());
}
