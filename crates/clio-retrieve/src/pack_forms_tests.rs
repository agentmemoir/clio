// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pack-forms unit tests: the selector
//! branches per fidelity channel, the budget fallback, the reference marker,
//! and the tally fold.
//!
//! ## Owns
//! - Unit tests for the `hit_form_text` selector branches, the
//!   `REFERENCE_MARKER` contract, and the `FormTally` fold
//!
//! ## Does not own
//! - The pack wiring that calls the selector and reports the tally
//!   (`compose_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Pure unit layer: fixed inline inputs, no fixtures, no host values.

use super::{FormTally, HitForm, REFERENCE_MARKER, hit_form_text};
use crate::token::estimate_tokens;
use clio_config::retention_levels::PackFidelityDefault;

/// `flow` selector with the whole budget free, which is what a pack with no
/// charge yet sees.
fn flow(
    gist: Option<&str>,
    render: Option<&str>,
    snapshot_ref: Option<&str>,
    item_id: &str,
) -> (HitForm, String) {
    hit_form_text(
        PackFidelityDefault::Flow,
        usize::MAX,
        gist,
        render,
        snapshot_ref,
        item_id,
    )
}

/// `exact` selector with an explicit budget left at this hit's turn.
fn exact(
    remaining: usize,
    gist: Option<&str>,
    render: Option<&str>,
    snapshot_ref: Option<&str>,
    item_id: &str,
) -> (HitForm, String) {
    hit_form_text(
        PackFidelityDefault::Exact,
        remaining,
        gist,
        render,
        snapshot_ref,
        item_id,
    )
}

#[test]
fn t01_gist_present_yields_prose_with_gist_text() {
    let (form, text) = flow(
        Some("User prefers tea over coffee."),
        None,
        Some("snap:7"),
        "itm_1",
    );
    assert_eq!(form, HitForm::Prose);
    assert_eq!(text, "User prefers tea over coffee.");
}

#[test]
fn t02_no_gist_with_snapshot_yields_marked_snapshot_reference() {
    let (form, text) = flow(None, None, Some("snapshot:9f2"), "itm_1");
    assert_eq!(form, HitForm::Reference);
    assert_eq!(text, format!("{REFERENCE_MARKER}snapshot:9f2"));
}

#[test]
fn t03_no_gist_no_snapshot_yields_marked_item_id() {
    let (form, text) = flow(None, None, None, "itm_abc");
    assert_eq!(form, HitForm::Reference);
    assert_eq!(text, format!("{REFERENCE_MARKER}itm_abc"));
}

#[test]
fn t05_reference_text_always_carries_marker_and_never_bare_identifier() {
    for (form, text) in [
        flow(None, None, Some("snapshot:9f2"), "itm_abc"),
        flow(None, None, None, "itm_abc"),
    ] {
        assert_eq!(form, HitForm::Reference);
        assert!(
            text.starts_with(REFERENCE_MARKER),
            "unmarked reference: {text}"
        );
        assert_ne!(text, "itm_abc", "bare item id leaked as text");
        assert_ne!(text, "snapshot:9f2", "bare snapshot ref leaked as text");
    }
}

#[test]
fn t10_exact_form_unreachable_without_authoritative_input() {
    for (form, _) in [
        flow(Some("gist"), None, Some("snapshot:9f2"), "itm_abc"),
        flow(Some("gist"), None, None, "itm_abc"),
        flow(None, None, Some("snapshot:9f2"), "itm_abc"),
        flow(None, None, None, "itm_abc"),
    ] {
        assert_ne!(
            form,
            HitForm::Exact,
            "exact must need an authoritative input"
        );
    }
}

#[test]
fn exact_form_selected_when_authoritative_rendering_supplied() {
    let (form, text) = flow(
        None,
        Some("verbatim quote"),
        Some("snapshot:9f2"),
        "itm_abc",
    );
    assert_eq!(form, HitForm::Exact);
    assert_eq!(text, "verbatim quote");
}

#[test]
fn empty_gist_or_authoritative_count_as_absent() {
    let (form, text) = flow(Some(""), Some(""), Some("snapshot:9f2"), "itm_abc");
    assert_eq!(form, HitForm::Reference);
    assert_eq!(text, format!("{REFERENCE_MARKER}snapshot:9f2"));
}

#[test]
fn marker_constant_is_the_documented_prefix() {
    assert_eq!(REFERENCE_MARKER, "[reference] ");
}

#[test]
fn marker_adds_a_real_token_charge() {
    let (_, marked) = flow(None, None, None, "itm_abc");
    assert!(estimate_tokens(&marked) > estimate_tokens("itm_abc"));
    assert!(estimate_tokens(&marked) > 0);
}

#[test]
fn hit_form_serializes_snake_case() {
    assert_eq!(serde_json::to_value(HitForm::Prose).unwrap(), "prose");
    assert_eq!(serde_json::to_value(HitForm::Exact).unwrap(), "exact");
    assert_eq!(
        serde_json::to_value(HitForm::Reference).unwrap(),
        "reference"
    );
    assert_eq!(HitForm::Prose.as_str(), "prose");
}

#[test]
fn tally_default_is_all_zero() {
    assert_eq!(
        FormTally::default(),
        FormTally {
            prose: 0,
            exact: 0,
            reference: 0
        }
    );
}

#[test]
fn tally_fold_counts_each_form_once() {
    let forms = [
        HitForm::Prose,
        HitForm::Reference,
        HitForm::Prose,
        HitForm::Exact,
        HitForm::Reference,
    ];
    let tally = FormTally::fold(forms.into_iter());
    assert_eq!(
        tally,
        FormTally {
            prose: 2,
            exact: 1,
            reference: 2
        }
    );
}

// The channel decides which half wins, and both channels end at the same
// reference. Every case below is the pure function over (render, gist).

#[test]
fn exact_channel_prefers_the_rendering_over_the_gist() {
    let (form, text) = exact(64, Some("the gist"), Some("the render"), None, "itm_1");
    assert_eq!(form, HitForm::Exact);
    assert_eq!(text, "the render");
}

#[test]
fn flow_channel_keeps_the_gist_ahead_of_the_rendering() {
    let (form, text) = flow(Some("the gist"), Some("the render"), None, "itm_1");
    assert_eq!(form, HitForm::Prose);
    assert_eq!(text, "the gist");
}

#[test]
fn exact_channel_falls_to_the_gist_without_a_rendering() {
    let (form, text) = exact(64, Some("the gist"), None, Some("snap:1"), "itm_1");
    assert_eq!(form, HitForm::Prose);
    assert_eq!(text, "the gist");
}

#[test]
fn exact_channel_falls_to_the_marked_reference_without_either_half() {
    let (form, text) = exact(64, None, None, Some("snap:1"), "itm_1");
    assert_eq!(form, HitForm::Reference);
    assert_eq!(text, format!("{REFERENCE_MARKER}snap:1"));
}

#[test]
fn an_empty_rendering_counts_as_absent_on_the_exact_channel() {
    let (form, text) = exact(64, Some("the gist"), Some(""), None, "itm_1");
    assert_eq!(form, HitForm::Prose);
    assert_eq!(text, "the gist");
}

// The budget fallback is the one decision point: the recorded form is the form
// of the returned text, and nothing is shortened to fit.

#[test]
fn a_rendering_that_exactly_fits_the_remaining_budget_is_kept() {
    let render = "x".repeat(estimate_tokens("abcd") * 4);
    let remaining = estimate_tokens(&render);
    let (form, text) = exact(remaining, Some("the gist"), Some(&render), None, "itm_1");
    assert_eq!(form, HitForm::Exact);
    assert_eq!(text, render, "a rendering that fits is never replaced");
}

#[test]
fn a_rendering_one_token_over_the_remaining_budget_falls_to_the_reference() {
    let render = "x".repeat(estimate_tokens("abcd") * 4);
    let remaining = estimate_tokens(&render) - 1;
    let (form, text) = exact(remaining, Some("the gist"), Some(&render), None, "itm_1");
    assert_eq!(
        form,
        HitForm::Reference,
        "the record must name the text used"
    );
    assert_eq!(text, format!("{REFERENCE_MARKER}itm_1"));
}

#[test]
fn the_flow_channel_ignores_the_budget_for_its_half_order() {
    let (form, text) = flow(Some("the gist"), Some("the render"), None, "itm_1");
    assert_eq!(form, HitForm::Prose);
    assert_eq!(text, "the gist");
    let (form, text) = hit_form_text(
        PackFidelityDefault::Flow,
        0,
        None,
        Some("the render"),
        None,
        "itm_1",
    );
    assert_eq!(form, HitForm::Exact, "flow never charges the budget here");
    assert_eq!(text, "the render");
}
