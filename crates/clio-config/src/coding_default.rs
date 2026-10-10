// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the builtin coding-agent default retention
//! profile and the standard chatter fixture used to calibrate it.
//!
//! ## Owns
//! - `coding_default_profile()`: first-run defaults for new coding-agent banks
//!   (`verbosity=selective`, `duplicate_tolerance_write=strict`, builtin mission)
//! - The pinned keep/drop example lists and the 20-line standard chatter fixture
//!
//! ## Does not own
//! - Mission schema, validation, or matching (see `retention_mission`)
//! - Where the default is applied during resolution (see `retention_store`)
//!
//! ## Boundary
//!
//! Changes to the builtin example lists or the fixture should normally require
//! changes here. Changes to profile storage or resolution order should not
//! require changes here.
//!
//! Keep this module focused on the coding default and its calibration fixture.

use crate::retention::{
    DEFAULT_RETENTION_FLOOR, DuplicateTolerance, PackFidelityDefault, RETENTION_PROFILE_VERSION,
    RecallScope, RetentionProfile, Verbosity,
};
use crate::retention_mission::{MissionEntry, MissionMatchKind};
use std::collections::BTreeMap;

/// Mission statement shipped with the coding default: durable reusable outcomes
/// in, routine chatter out, one compact summary over transcript detail. Must
/// stay at or below `MAX_MISSION_STATEMENT_CHARS` (500).
pub const CODING_MISSION_STATEMENT: &str = "Retain durable, reusable outcomes likely to matter again: user preferences, decision boundaries, operating constraints, verified conclusions, root-cause findings, stable config facts, recurring workflow rules, notable people and projects, important communications, and actionable tool outputs. Drop conversational filler, duplicate paraphrases, routine status chatter, unverified speculation, and debug noise without a durable lesson. When in doubt, keep one compact high-signal summary.";

/// Keep list for the coding default: durable, reusable outcomes.
const CODING_KEEP_EXAMPLES: &[&str] = &[
    "prefers",
    "decision",
    "decided",
    "constraint",
    "must always",
    "root cause",
    "verified",
    "config",
    "workflow rule",
    "follow-up",
    "alert",
    "environment",
];

/// Drop list for the coding default: routine status chatter the bank must not
/// store. Bare test-result lines, PR/issue-created notices without decisions,
/// repeated identity restatements, heartbeat/command dumps, and filler.
const CODING_DROP_EXAMPLES: &[&str] = &[
    "tests passed",
    "test run passed",
    "pr created",
    "pull request created",
    "issue created",
    "user name is",
    "my name is",
    "heartbeat",
    "command dump",
    "thanks",
    "sounds good",
    "got it",
    "let me know",
    "on it",
    "in progress",
];

fn literal_entries(texts: &[&str], reject: bool) -> Vec<MissionEntry> {
    texts
        .iter()
        .map(|text| MissionEntry {
            text: (*text).to_owned(),
            match_kind: MissionMatchKind::Literal,
            reject,
        })
        .collect()
}

/// The builtin `coding_default_profile` applied to new coding-agent banks:
/// `verbosity=selective`, `duplicate_tolerance_write=strict`, and the builtin
/// mission. Passing an operator profile to `retention_profile_set` replaces the
/// whole mission; this default never mutates an existing bank.
#[must_use]
pub fn coding_default_profile() -> RetentionProfile {
    RetentionProfile {
        verbosity: Verbosity::Selective,
        theta_offsets: BTreeMap::new(),
        duplicate_tolerance_write: DuplicateTolerance::Strict,
        duplicate_tolerance_read: DuplicateTolerance::Balanced,
        recall_scope_default: RecallScope::Full,
        mission: Some(crate::retention_mission::RetentionMission {
            mission_statement: CODING_MISSION_STATEMENT.to_owned(),
            keep_examples: literal_entries(CODING_KEEP_EXAMPLES, false),
            drop_examples: literal_entries(CODING_DROP_EXAMPLES, true),
        }),
        retention_curve: true,
        retention_floor: DEFAULT_RETENTION_FLOOR,
        pack_fidelity_default: PackFidelityDefault::Flow,
        provenance_floor_default: None,
        maintenance_trigger: true,
        version: RETENTION_PROFILE_VERSION,
    }
}

/// The standard 20-line chatter fixture: 4 durable lines then 16 routine
/// status/filler lines. Under the coding default the 4 durable lines are
/// admitted and the 16 chatter lines are rejected with `mission_drop` rule ids;
/// under an empty mission with identical verbosity every line is scored on the
/// five factors alone. Calibration evidence is published per phase.
pub const CHATTER_FIXTURE: [&str; 20] = [
    // Durable (4): admitted under the coding default.
    "User prefers tabs over spaces in this project.",
    "Decision: the store module must use strict duplicate tolerance.",
    "Constraint: the deploy script must never touch production.",
    "Root cause found: the retry loop must serialize access to the session file.",
    // Routine status and filler (16): rejected under the coding default.
    "Tests passed",
    "All tests passed",
    "Test run passed for the full suite",
    "PR created",
    "PR created for the retry-loop branch",
    "Pull request created",
    "Issue created",
    "Issue created for the retry loop",
    "User name is Andy",
    "My name is Andy",
    "Heartbeat ok",
    "Heartbeat ping at 09:00",
    "Command dump written to the log",
    "Thanks!",
    "Sounds good, let me know",
    "Got it, will update later",
];

#[cfg(test)]
#[path = "coding_default_tests.rs"]
mod coding_default_tests;
