// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the styled fragments: what each
//! one says, and the promise that a colour-off run prints the bare text.
//!
//! ## Owns
//! - Render behavior under test for `setup_style`
//!
//! ## Does not own
//! - The prompt shapes and their bytes (see `setup_render_tests`)
//! - The secret-reading seam (see `setup_mask_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to what a fragment says should normally update tests here. Changes to
//! a prompt or to a question should not require changes here.
//!
//! Keep this module focused on the styled fragments.

use super::*;
use crate::cli_style::Palette;

#[test]
fn colour_off_output_is_the_bare_text() {
    // Every golden assertion in the wizard's suites depends on this: with colour
    // off the palette returns its input untouched, so a piped run, a `NO_COLOR`
    // run, and every test see plain text.
    let palette = Palette::new(false);
    assert_eq!(title(palette), "Clio setup");
    assert_eq!(section(palette, "Database"), "Database");
    assert_eq!(hint(palette, "5432"), "[5432]");
    assert_eq!(confirm(palette, "ok"), "ok");
    assert_eq!(warn(palette, "careful"), "careful");
    assert_eq!(error(palette, "no"), "no");
}

#[test]
fn colour_on_output_wraps_the_text_and_resets() {
    // The counter-check on the test above: a palette that never painted would
    // satisfy it too. The reset has to be last, or the colour would stop before
    // the last character is printed.
    let palette = Palette::new(true);
    let painted = title(palette);
    assert!(painted.starts_with('\u{1b}'), "{painted:?}");
    assert!(painted.contains("Clio setup"), "{painted:?}");
    assert!(painted.ends_with("\u{1b}[0m"), "{painted:?}");
}

#[test]
fn the_default_hint_brackets_the_value() {
    // A default shown without brackets is a second question rather than a
    // suggestion, so the bracket is part of the fragment's contract.
    let palette = Palette::new(false);
    assert_eq!(hint(palette, "require"), "[require]");
    assert_eq!(hint(palette, ""), "[]");
}

#[test]
fn the_going_back_line_names_the_section_and_the_quit_line_says_nothing_ran() {
    assert_eq!(
        going_back("Embedding"),
        "Going back. Your answers to Embedding will be discarded."
    );
    assert_eq!(
        quitting(),
        "Nothing has been started and nothing has been written."
    );
    assert_eq!(
        nothing_to_undo(),
        "Nothing has been started and nothing has been written yet."
    );
}

#[test]
fn the_two_abandon_lines_differ_only_by_the_trailing_yet() {
    // They are two halves of one moment. If they were ever made the same string
    // the operator would lose the distinction between "we are going back" and
    // "we are stopping", which is the only thing the second line explains.
    assert_ne!(quitting(), nothing_to_undo());
    assert!(nothing_to_undo().ends_with("yet."));
    assert!(!quitting().ends_with("yet."));
}
