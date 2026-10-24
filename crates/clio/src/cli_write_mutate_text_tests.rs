// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the human text views the mutation verbs print off the JSON
//! path: the exact sentence `update`, `invalidate`, and `discard` emit.
//!
//! ## Owns
//! - Text-mode output assertions for the mutation write verbs
//!
//! ## Does not own
//! - Their JSON payloads, counts, and exit codes (see `cli_write_mutate_tests`)
//! - The commands under test (`cli_write_mutate`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{seed_item, state};
use crate::cli_write_test_support::run_write_state;
use std::process::ExitCode;

#[test]
fn invalidate_text_mode_view_without_replacement() {
    let s = state();
    seed_item(&s, "demo", "itm-i9", "value");
    let (code, out, err) = run_write_state("invalidate", &["itm-i9", "--bank", "demo"], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        out.trim(),
        "invalidated (closed 1, already_ended 0) in bank `demo`",
        "{out}"
    );
}

#[test]
fn update_text_mode_view_is_deterministic() {
    let s = state();
    seed_item(&s, "demo", "itm-u5", "value");
    let (code, out, err) = run_write_state(
        "update",
        &["itm-u5", "ignored", "--rule", "discrete", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.starts_with("updated (discrete): closed 1"), "{out}");
}

#[test]
fn discard_text_mode_view_reports_the_removed_item() {
    let s = state();
    seed_item(&s, "demo", "itm-d5", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d5", "--reason", "noise", "--confirm", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(out.trim(), "discarded itm-d5 in bank `demo`", "{out}");
}

#[test]
fn discard_dry_run_text_view_states_irreversibility_note() {
    let s = state();
    seed_item(&s, "demo", "itm-d6", "value");
    let (code, out, err) = run_write_state(
        "discard",
        &["itm-d6", "--reason", "noise", "--dry-run", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("would discard itm-d6"), "{out}");
    assert!(out.contains("not compliance erasure"), "{out}");
}
