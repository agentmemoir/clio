// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the bank retention mission: structured
//! keep/drop example lists with deterministic admission influence.
//!
//! ## Owns
//! - `RetentionMission`, `MissionEntry`, and `MissionMatchKind` schema and validation
//! - The specificity lint, size caps, duplicate rejection, secret rejection, and pinned-subset regex checks
//! - The resolved mission match engine and re-exports of the rule-id helpers
//! - The exact utility arithmetic: drop hit `−0.30`, keep hit `+0.15`, drop wins
//!
//! ## Does not own
//! - The retention profile object that carries the mission (see `retention`)
//! - The pattern grammar and bounded matcher internals (see `mission_match`)
//! - Admission scoring that consumes the evaluation (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to mission fields, validation, ids, or arithmetic should normally
//! require changes here. Changes to profile storage or factor formulas should not
//! require changes here.
//!
//! Keep this module focused on the mission object and its deterministic matching.
//!
//! ## Rule ids
//!
//! A rule id is the hex encoding of FNV-1a 64 over the Unicode-lowercased,
//! NFC-normalized entry text, a `\u{1}` separator, and the `match` discriminator
//! (`literal` or `regex`). Ids are stable across mission edits that do not touch
//! the entry. Literal matching is case-insensitive substring; regex entries use
//! the pinned subset and are matched case-insensitively.
//!
//! ## Utility arithmetic (frozen for first release)
//!
//! | Hit | Utility adjustment | Rule |
//! |-----|--------------------|------|
//! | Drop-example hit | `−0.30` | Drop wins over keep; a `reject=true` drop entry sets a `mission_drop:<id>` rejection |
//! | Keep-example hit | `+0.15` | Ignored when a drop entry also hits; never rescues a gate failure |

use crate::error::{AmError, ErrorCode};
use crate::log;
use crate::mission_match::{self, MissionMatchResult, MissionPattern};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

#[path = "mission_rule.rs"]
pub(crate) mod mission_rule;
use mission_rule::{mission_rule_id, mission_rule_key};

/// Longest allowed `mission_statement`.
pub const MAX_MISSION_STATEMENT_CHARS: usize = 500;

/// Most entries a single example list may carry.
pub const MAX_MISSION_EXAMPLES_PER_LIST: usize = 20;

/// Longest allowed single example entry.
pub const MAX_MISSION_ENTRY_CHARS: usize = 200;

/// Cap on combined mission text so per-write matching stays `O(examples)`.
pub const MAX_MISSION_TOTAL_CHARS: usize = 4096;

/// Utility penalty applied on a drop-example hit.
pub const MISSION_DROP_PENALTY: f64 = -0.30;

/// Utility bonus applied on a keep-example hit (never with a drop hit).
pub const MISSION_KEEP_BONUS: f64 = 0.15;

/// Minimum chars for the specificity lint; missions whose examples are ALL
/// shorter than this (or ALL generic stopwords) are rejected.
const MIN_SPECIFIC_ENTRY_CHARS: usize = 3;

/// Generic stopwords rejected by the specificity lint when an ENTIRE mission's
/// examples consist only of these.
const GENERIC_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "into", "data", "info", "log", "logs",
    "error", "errors", "status", "note", "notes", "misc", "other", "stuff", "things", "update",
    "updates", "text", "line", "lines",
];

fn default_true() -> bool {
    true
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionMatchKind {
    /// Case-insensitive substring (default).
    #[default]
    Literal,
    /// Pinned-subset regex (literals, character classes, `+`/`*`/`?`, `|`).
    Regex,
}

impl MissionMatchKind {
    /// Wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Regex => "regex",
        }
    }
}

/// One keep/drop example: a literal substring or a documented regex opt-in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionEntry {
    /// Entry text: a substring, or a pinned-subset pattern when `match` is `regex`.
    pub text: String,
    /// Match discriminator; `literal` is the case-insensitive default.
    #[serde(default, rename = "match")]
    pub match_kind: MissionMatchKind,
    /// Drop entries only: a hit rejects the write outright (default `true`).
    /// Keep entries ignore this field.
    #[serde(default = "default_true")]
    pub reject: bool,
}

/// Structured bank retention mission: what to keep and what to drop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionMission {
    /// Human-readable mission statement (≤500 chars).
    #[serde(default)]
    pub mission_statement: String,
    /// Patterns whose hits protect a candidate's utility score.
    #[serde(default)]
    pub keep_examples: Vec<MissionEntry>,
    /// Patterns whose hits penalize or reject a candidate. At least one is required.
    #[serde(default)]
    pub drop_examples: Vec<MissionEntry>,
}

impl RetentionMission {
    /// Validate the mission: caps, at least one drop example, no duplicates, no
    /// secret-bearing entries, regex entries in the pinned subset, and the
    /// specificity lint.
    ///
    /// # Errors
    /// An oversized statement or entry, over-long lists, a missing drop list,
    /// duplicate entries, secret-bearing entries, an invalid regex, a lint
    /// failure, or an over-large mission.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.mission_statement.chars().count() > MAX_MISSION_STATEMENT_CHARS {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "mission_statement must be at most {MAX_MISSION_STATEMENT_CHARS} characters"
                ),
            ));
        }
        if self.mission_statement.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "mission_statement is required when a mission is set",
            ));
        }
        if self.drop_examples.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "a mission requires at least one drop example (what to stop storing)",
            ));
        }
        for (list_name, list) in [
            ("keep_examples", &self.keep_examples),
            ("drop_examples", &self.drop_examples),
        ] {
            if list.len() > MAX_MISSION_EXAMPLES_PER_LIST {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("{list_name} exceeds {MAX_MISSION_EXAMPLES_PER_LIST} entries"),
                ));
            }
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut total_chars = 0usize;
        for entry in self.keep_examples.iter().chain(&self.drop_examples) {
            let len = entry.text.chars().count();
            if len == 0 {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "example entries must be non-empty",
                ));
            }
            if len > MAX_MISSION_ENTRY_CHARS {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("example entries must be at most {MAX_MISSION_ENTRY_CHARS} characters"),
                ));
            }
            total_chars += len;
            let scrubbed = crate::secret::scrub_inline_secrets(&entry.text);
            if scrubbed != entry.text {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "example entry `{scrubbed}` looks like it carries secret material; store only pattern text and scrub real credentials with the hygiene tooling (hygiene_audit / hygiene_clean)"
                    ),
                ));
            }
            if entry.match_kind == MissionMatchKind::Regex
                && let Err(reason) = mission_match::compile(&entry.text)
            {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("regex entry `{}` is invalid: {reason}", entry.text),
                ));
            }
            if !seen.insert(mission_rule_key(entry)) {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("duplicate example entry `{}`", entry.text),
                ));
            }
        }
        if total_chars > MAX_MISSION_TOTAL_CHARS {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!("combined mission text exceeds {MAX_MISSION_TOTAL_CHARS} characters"),
            ));
        }
        self.specificity_lint()?;
        Ok(())
    }

    /// Reject missions whose examples are all shorter than 3 chars or all
    /// generic stopwords, with a good example in the message.
    fn specificity_lint(&self) -> Result<(), AmError> {
        let entries: Vec<&MissionEntry> = self
            .keep_examples
            .iter()
            .chain(self.drop_examples.iter())
            .collect();
        let all_short = entries
            .iter()
            .all(|e| e.text.chars().count() < MIN_SPECIFIC_ENTRY_CHARS);
        let all_generic = entries.iter().all(|e| {
            let lower = e.text.to_lowercase();
            GENERIC_STOPWORDS.contains(&lower.as_str())
        });
        if all_short || all_generic {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "mission examples are too generic to steer admission; name concrete patterns, e.g. drop `tests passed` (bare test-result lines) and `pr created` (status notices), keep `prefers` (user preferences)",
            ));
        }
        Ok(())
    }
}

/// One compiled mission entry, ready to match candidate text.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMissionEntry {
    /// Stable content-hash id.
    pub rule_id: String,
    /// Secret-scrubbed entry text for preview display.
    pub label: String,
    /// True for drop-list entries.
    pub is_drop: bool,
    /// Drop entries only: a hit rejects the write outright.
    pub reject: bool,
    pub(crate) matcher: CompiledMatcher,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CompiledMatcher {
    /// Case-insensitive substring over the lowercased candidate text.
    Literal(String),
    /// Pinned-subset pattern over the lowercased candidate text.
    Regex(MissionPattern),
}

/// Resolved mission snapshot carried inside a resolved retention profile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedMission {
    entries: Vec<ResolvedMissionEntry>,
}

impl ResolvedMission {
    /// Compile a mission for matching; a mission that fails validation here
    /// degrades to an empty mission with a warning (never fails writes open).
    #[must_use]
    pub fn from_mission(mission: Option<&RetentionMission>) -> Self {
        let Some(mission) = mission else {
            return Self::default();
        };
        if let Err(e) = mission.validate() {
            log::warn(
                "mission invalid at resolution; treating as empty",
                &[("reason", e.message.as_str())],
            );
            return Self::default();
        }
        let mut entries = Vec::new();
        for entry in mission.keep_examples.iter().chain(&mission.drop_examples) {
            let is_drop = mission.drop_examples.contains(entry);
            let matcher = match entry.match_kind {
                MissionMatchKind::Literal => CompiledMatcher::Literal(entry.text.to_lowercase()),
                MissionMatchKind::Regex => match mission_match::compile(&entry.text) {
                    Ok(pattern) => CompiledMatcher::Regex(pattern),
                    // Validated above; unreachable, but degrade rather than fail writes.
                    Err(_) => continue,
                },
            };
            entries.push(ResolvedMissionEntry {
                rule_id: mission_rule_id(entry),
                label: crate::secret::scrub_inline_secrets(&entry.text),
                is_drop,
                reject: is_drop && entry.reject,
                matcher,
            });
        }
        Self { entries }
    }

    /// True when the mission carries no entries (behavior-neutral).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Deterministic mission evaluation over candidate text (any case).
    ///
    /// Drop hits win over keep hits: the utility delta is the drop penalty and
    /// `drop_reject_id` carries the first hitting reject-declared drop rule.
    #[must_use]
    pub fn evaluate(&self, text: &str) -> MissionEvaluation {
        let mut evaluation = MissionEvaluation::default();
        if self.entries.is_empty() {
            return evaluation;
        }
        let lower = text.to_lowercase();
        let lower_chars: Vec<char> = if self
            .entries
            .iter()
            .any(|e| matches!(e.matcher, CompiledMatcher::Regex(_)))
        {
            lower.chars().collect()
        } else {
            Vec::new()
        };
        let mut drop_hit = false;
        for entry in &self.entries {
            let matched = match &entry.matcher {
                CompiledMatcher::Literal(needle) => lower.contains(needle.as_str()),
                CompiledMatcher::Regex(pattern) => {
                    match mission_match::is_match(pattern, &lower_chars) {
                        MissionMatchResult::Matched => true,
                        MissionMatchResult::NoMatch => false,
                        MissionMatchResult::BudgetExhausted => {
                            log::warn(
                                "mission regex evaluation exceeded its step budget; treated as non-match",
                                &[("rule_id", entry.rule_id.as_str())],
                            );
                            false
                        }
                    }
                }
            };
            if !matched {
                continue;
            }
            evaluation.rule_ids.push(entry.rule_id.clone());
            evaluation.rule_labels.push(entry.label.clone());
            if entry.is_drop {
                drop_hit = true;
                if entry.reject && evaluation.drop_reject_id.is_none() {
                    evaluation.drop_reject_id = Some(entry.rule_id.clone());
                }
            }
        }
        evaluation.utility_delta = if drop_hit {
            MISSION_DROP_PENALTY
        } else if !evaluation.rule_ids.is_empty() {
            MISSION_KEEP_BONUS
        } else {
            0.0
        };
        evaluation
    }
}

/// Deterministic mission evaluation result for one gated write.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MissionEvaluation {
    /// All matched rule ids (keep and drop), in entry order.
    pub rule_ids: Vec<String>,
    /// Secret-scrubbed labels parallel to `rule_ids`.
    pub rule_labels: Vec<String>,
    /// Utility adjustment: `−0.30` on a drop hit, `+0.15` on a keep-only hit,
    /// `0.0` otherwise.
    pub utility_delta: f64,
    /// Rule id of the first hitting `reject=true` drop entry (veto reason).
    pub drop_reject_id: Option<String>,
}

/// Log the coding-default application once per bank per process.
///
/// # Panics
/// Panics if the log-once mutex is poisoned (a bug, not a runtime condition).
pub fn log_coding_default_once(bank: &str, profile_version: u32) {
    static LOGGED: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    let logged = LOGGED.get_or_init(|| std::sync::Mutex::new(HashSet::new()));
    let mut seen = logged.lock().expect("coding default log mutex poisoned");
    if seen.insert(bank.to_owned()) {
        log::info(
            "coding_default_profile_applied",
            &[
                ("bank", bank),
                ("profile_version", &profile_version.to_string()),
            ],
        );
    }
}

#[cfg(test)]
#[path = "retention_mission_tests.rs"]
mod retention_mission_tests;

#[cfg(test)]
#[path = "retention_mission_spike_tests.rs"]
mod retention_mission_spike_tests;
