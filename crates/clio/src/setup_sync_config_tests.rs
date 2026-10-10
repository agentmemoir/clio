// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the single-host predicate: which
//! `/sync/omit` shapes read as single-host and which do not.
//!
//! ## Owns
//! - Every test of `setup_sync_config::single_host`
//!
//! ## Does not own
//! - What the wizard asks when the answer is false (see
//!   `setup_steps_sync_tests`)
//! - What `clio sync status` does on a single-host deployment (see
//!   `sync_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Every case is a literal config value, so no test reads the machine's
//! own config file and the answers are the same everywhere.
//!
//! Keep this module focused on the predicate.

use super::*;
use serde_json::json;

#[test]
fn an_absent_pointer_reads_as_single_host() {
    assert!(single_host(&json!({})), "no sync section at all");
    assert!(
        single_host(&json!({ "sync": {} })),
        "a sync section with no omit key"
    );
}

#[test]
fn an_explicit_omit_reads_as_single_host() {
    assert!(single_host(&json!({ "sync": { "omit": true } })));
}

#[test]
fn sync_kept_in_play_reads_as_multi_host() {
    assert!(
        !single_host(&json!({ "sync": { "omit": false } })),
        "a deployment that enabled sync is not single-host"
    );
}

#[test]
fn a_non_boolean_omit_reads_as_single_host() {
    // Absent, `true`, and unusable all fall the same way, so a config a
    // hand-edited value cannot express behaves like a fresh one rather than
    // silently becoming multi-host.
    for value in [json!("yes"), json!(1), json!(null), json!({})] {
        assert!(
            single_host(&json!({ "sync": { "omit": value } })),
            "an unusable omit value must not claim sync is in play"
        );
    }
}
