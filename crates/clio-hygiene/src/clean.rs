// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the confirmed `hygiene_clean` mutator:
//! preview counting without `confirm` (zero writes, zero durable rows),
//! confirmed `flag`/`archive`/`discard` (`delete` alias) with per-item
//! durable hygiene-log rows, and the explicit never-erase boundary.
//!
//! ## Owns
//! - [`clean`]: target resolution, confirmation gate, per-item executors,
//!   batch abort vs skip semantics, and log-row persistence
//! - [`CleanAction`]: the closed action set; `delete` is an alias of `discard`
//!   documented as non-compliance (never `erase_request` / DEK destroy)
//! - The presenting credential's label and the stated reason/note inside each
//!   durable log row's detail
//!
//! ## Does not own
//! - Store SQL (flags, archive markers, discard columns — `clio-store`)
//! - Scoring or candidate ranking (`scoring` / `audit`)
//!
//! ## Boundary
//!
//! Changes to the confirmation gate, action execution, or log-row writes should
//! normally require changes here. Changes to store transactions should not
//! require changes here.
//!
//! Keep this module focused on the clean orchestration contract.

use crate::audit::NoiseCandidate;
use clio_store::{HygieneLogEntry, HygieneStore, Store};
use clio_types::{AmError, ErrorCode, OpContext, record_principal};
use serde::Serialize;

/// One clean target: an item id plus optional audit metadata (`apply_suggested`
/// needs the suggested action; scores land in the log rows when present).
#[derive(Debug, Clone, PartialEq)]
pub struct CleanTarget {
    /// Item id (content is never accepted in clean requests).
    pub item_id: String,
    /// Noise score from the prior audit, when known.
    pub noise_score: Option<f64>,
    /// Suggested action from the prior audit (required by `apply_suggested`).
    pub suggested_action: Option<String>,
}

/// Closed clean action set. `Delete` maps to `Discard` and is
/// documented as a non-compliance alias: it never touches DEKs or tombstones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanAction {
    /// Mark for review; item stays in retrieve.
    Flag,
    /// Reversible exclusion from default retrieve (`hygiene_archive`).
    Archive,
    /// Operations removal via the existing discard path (not compliance erase).
    Discard,
    /// Apply each target's `suggested_action`; a `keep` suggestion is skipped.
    ApplySuggested,
}

impl CleanAction {
    /// Parse a caller action label (`delete` accepted as `discard` alias).
    ///
    /// # Errors
    /// Unknown action labels.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "flag" => Ok(Self::Flag),
            "archive" => Ok(Self::Archive),
            "discard" | "delete" => Ok(Self::Discard),
            "apply_suggested" => Ok(Self::ApplySuggested),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "hygiene action must be flag|archive|discard|delete|apply_suggested, got `{other}`"
                ),
            )),
        }
    }

    /// Wire label used in hygiene log rows (the resolved mutation).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Archive => "archive",
            Self::Discard => "discard",
            Self::ApplySuggested => "apply_suggested",
        }
    }
}

/// Inputs for [`clean`].
pub struct CleanArgs<'a> {
    /// Discrete repository (identity resolution + discard path).
    pub store: &'a dyn Store,
    /// Hygiene persistence (flags, archive markers, durable log).
    pub hygiene: &'a dyn HygieneStore,
    /// Bank / actor context (hygiene respects caller scope).
    pub ctx: &'a OpContext,
    /// Targets: prior audit candidates or bare ids.
    pub targets: &'a [CleanTarget],
    /// Requested action (`apply_suggested` resolves per-target suggestions).
    pub action: CleanAction,
    /// `false` (default) = dry-run counts only; `true` = mutate.
    pub confirm: bool,
    /// Non-empty removal reason, required by the confirmed `discard` path.
    pub reason: &'a str,
    /// Effective timestamp for archive/discard stamps and log rows.
    pub timestamp: &'a str,
    /// Invocation id shared by all log rows of one clean call.
    pub batch_id: &'a str,
    /// Review note persisted on `flag` (optional).
    pub note: Option<&'a str>,
}

/// Outcome of one `hygiene_clean` call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CleanOutcome {
    /// True when nothing was written (dry-run).
    pub dry_run: bool,
    /// Targets this run resolves to `flag`: what a dry-run would apply, equal
    /// to the applied flag count on a confirmed run.
    pub would_flag: usize,
    /// Targets this run resolves to `archive`: what a dry-run would apply,
    /// equal to the applied archive count on a confirmed run.
    pub would_archive: usize,
    /// Targets this run resolves to `discard`: what a dry-run would apply,
    /// equal to the applied discard count on a confirmed run.
    pub would_discard: usize,
    /// Confirmed: item ids mutated so far.
    pub applied: Vec<String>,
    /// Confirmed: item ids skipped because their suggestion is `keep`.
    /// The batch continued without them.
    pub skipped: Vec<String>,
    /// Confirmed abort: targets not attempted after the first failure.
    pub pending: Vec<String>,
    /// Confirmed: per-item failure that aborted the batch.
    pub failed: Option<FailedTarget>,
}

/// One per-item failure with its error surface.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FailedTarget {
    /// Item id whose action failed.
    pub item_id: String,
    /// Error code + message (no content).
    pub error: String,
}

/// Run one `hygiene_clean` call.
///
/// Without `confirm`: zero writes and zero durable rows; returns
/// would-flag / would-archive / would-discard counts only, skipping targets
/// already discarded or archived. With `confirm`: every mutation writes its
/// telemetry row inside the store transaction and appends a durable
/// `hygiene_audit_logs` row, and a first per-item failure aborts the
/// remaining targets (reported as `pending`). A confirmed `apply_suggested`
/// run skips a target suggested as `keep` (nothing to apply; reported as
/// `skipped`) instead of failing the batch. The `would_*` counters keep one
/// meaning in both modes: on a confirmed run each equals the applied count
/// for its action. The action set never reaches compliance erasure: no DEK
/// destruction or tombstone write exists on this path.
///
/// # Errors
/// Missing bank, empty targets, an `apply_suggested` target with no
/// applicable suggested action (other than `keep`), empty ids, a state probe
/// failure, unconfirmed discard without a reason, or a log write failure.
pub fn clean(args: &CleanArgs<'_>) -> Result<CleanOutcome, AmError> {
    if args.ctx.bank.as_str().is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    if args.targets.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "at least one target item_id is required",
        ));
    }
    // Preflight: the confirmed discard path needs a non-empty reason.
    // `apply_suggested` resolves per-target suggestions, so it must reject too
    // when any target resolves to discard — before any mutation is applied.
    if args.confirm && confirmed_discard_requested(args) && args.reason.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "reason is required and must be non-empty for confirmed discard",
        ));
    }

    if !args.confirm {
        return preview_outcome(args);
    }

    let mut outcome = CleanOutcome {
        dry_run: false,
        would_flag: 0,
        would_archive: 0,
        would_discard: 0,
        applied: Vec::new(),
        skipped: Vec::new(),
        pending: Vec::new(),
        failed: None,
    };
    for (index, target) in args.targets.iter().enumerate() {
        validate_target_id(target.item_id.as_str())?;
        // A `keep`-suggestion target is skipped: nothing applies to it.
        let Some(action) = resolved_action(args.action, target)? else {
            outcome.skipped.push(target.item_id.clone());
            continue;
        };
        let result = match action {
            CleanAction::Flag => args
                .hygiene
                .set_hygiene_flag(
                    args.ctx.bank.as_str(),
                    &target.item_id,
                    args.note,
                    args.ctx.actor.as_str(),
                    args.timestamp,
                )
                .map(|_| ()),
            CleanAction::Archive => args
                .hygiene
                .archive_item(
                    args.ctx.bank.as_str(),
                    &target.item_id,
                    args.timestamp,
                    args.ctx.actor.as_str(),
                )
                .map(|_| ()),
            CleanAction::Discard => args
                .store
                .discard_item(
                    args.ctx.bank.as_str(),
                    &target.item_id,
                    args.reason.trim(),
                    args.timestamp,
                    args.ctx.actor.as_str(),
                )
                .map(|_| ()),
            CleanAction::ApplySuggested => Ok(()),
        };
        match result {
            Ok(()) => {
                // The same counters carry the same meaning as in a preview:
                // on a confirmed run each would_* equals the applied count
                // for its action, so a reader of either response can tell
                // what the run did. Failed and pending targets are not
                // counted.
                match action {
                    CleanAction::Flag => outcome.would_flag += 1,
                    CleanAction::Archive => outcome.would_archive += 1,
                    CleanAction::Discard => outcome.would_discard += 1,
                    CleanAction::ApplySuggested => {}
                }
                outcome.applied.push(target.item_id.clone());
                append_log(args, index, target, action)?;
            }
            Err(err) => {
                // Batch abort on first failure: remaining targets are reported
                // as pending, never half-applied silently.
                outcome.failed = Some(FailedTarget {
                    item_id: target.item_id.clone(),
                    error: format!("{}: {}", err.code, err.message),
                });
                for rest in &args.targets[index + 1..] {
                    outcome.pending.push(rest.item_id.clone());
                }
                break;
            }
        }
    }
    Ok(outcome)
}

/// True when this clean call mutates or resolves any target to `discard`
/// (explicit action, or `apply_suggested` over a target suggested as
/// `discard`).
fn confirmed_discard_requested(args: &CleanArgs<'_>) -> bool {
    args.action == CleanAction::Discard
        || (args.action == CleanAction::ApplySuggested
            && args
                .targets
                .iter()
                .any(|t| t.suggested_action.as_deref() == Some("discard")))
}

/// Resolve the per-target action (`apply_suggested` needs an audit
/// suggestion). `None` marks a target suggested as `keep`: nothing applies,
/// so the caller skips it rather than aborting the batch. Other unmapped
/// suggestions still fail closed.
fn resolved_action(
    action: CleanAction,
    target: &CleanTarget,
) -> Result<Option<CleanAction>, AmError> {
    match action {
        CleanAction::ApplySuggested => match target.suggested_action.as_deref() {
            Some("flag") => Ok(Some(CleanAction::Flag)),
            Some("archive") => Ok(Some(CleanAction::Archive)),
            Some("discard") => Ok(Some(CleanAction::Discard)),
            Some("keep") => Ok(None),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "target `{}` has no applicable suggested_action (`{other:?}`); supply an explicit action",
                    target.item_id
                ),
            )),
        },
        explicit => Ok(Some(explicit)),
    }
}

fn validate_target_id(item_id: &str) -> Result<(), AmError> {
    if item_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item_id is required for every clean target",
        ));
    }
    Ok(())
}

/// Append one durable hygiene-log row for a confirmed mutation. Write
/// failures propagate (fail closed: a cleanup that cannot be logged fails
/// rather than proceeding silently). The row id carries the target index so
/// duplicate target item ids never collide on the log primary key.
fn append_log(
    args: &CleanArgs<'_>,
    index: usize,
    target: &CleanTarget,
    action: CleanAction,
) -> Result<(), AmError> {
    let mut detail = serde_json::json!({
        "result": "applied",
        "batch_id": args.batch_id
    });
    // A bound transport names the credential beside the claimed role. A local
    // one has none, so the key is absent rather than empty.
    record_principal(&mut detail, args.ctx.principal());
    // The stated reason and note ride the same row, the way the audit event
    // carries them: present when supplied, absent rather than empty otherwise.
    let reason = args.reason.trim();
    if !reason.is_empty() {
        detail["reason"] = serde_json::json!(reason);
    }
    if let Some(note) = args.note {
        detail["note"] = serde_json::json!(note);
    }
    let entry = HygieneLogEntry {
        id: format!("hlg-{}-{}-{}", args.batch_id, index, target.item_id),
        bank_id: args.ctx.bank.as_str().to_owned(),
        item_id: target.item_id.clone(),
        noise_score: target.noise_score,
        action: action.as_str().to_owned(),
        actor: args.ctx.actor.as_str().to_owned(),
        confirmed: args.confirm,
        dry_run: false,
        detail,
        created_at: args.timestamp.to_owned(),
    };
    args.hygiene.insert_hygiene_log(&entry)
}

/// The preview branch: count the actionable targets and write nothing.
/// Items already discarded or archived have nothing left to apply, so they
/// contribute no count and no candidate action.
fn preview_outcome(args: &CleanArgs<'_>) -> Result<CleanOutcome, AmError> {
    let mut outcome = CleanOutcome {
        dry_run: true,
        would_flag: 0,
        would_archive: 0,
        would_discard: 0,
        applied: Vec::new(),
        skipped: Vec::new(),
        pending: Vec::new(),
        failed: None,
    };
    for target in args.targets {
        validate_target_id(target.item_id.as_str())?;
        // A `keep` suggestion has nothing to count: the preview skips it.
        let Some(action) = resolved_action(args.action, target)? else {
            continue;
        };
        if target_already_settled(args, &target.item_id)? {
            continue;
        }
        match action {
            CleanAction::Flag => outcome.would_flag += 1,
            CleanAction::Archive => outcome.would_archive += 1,
            CleanAction::Discard => outcome.would_discard += 1,
            CleanAction::ApplySuggested => {}
        }
    }
    Ok(outcome)
}

/// True when the item already sits in the end state a clean targets: it is
/// discarded, or it carries the archive marker. A preview reports nothing to
/// do for such a target.
fn target_already_settled(args: &CleanArgs<'_>, item_id: &str) -> Result<bool, AmError> {
    let bank = args.ctx.bank.as_str();
    let active = args.store.get_item_identity(bank, item_id)?.is_some();
    if active {
        return args.store.item_is_archived(bank, item_id);
    }
    // A discarded row keeps its memory row; a missing one has none. Missing
    // items stay reportable: the confirmed path surfaces them as failures.
    Ok(args.store.get_memory_item(bank, item_id)?.is_some())
}

/// Build clean targets straight from audit candidates (MCP bridge).
#[must_use]
pub fn targets_from_candidates(candidates: &[NoiseCandidate]) -> Vec<CleanTarget> {
    candidates
        .iter()
        .map(|c| CleanTarget {
            item_id: c.item_id.clone(),
            noise_score: Some(c.noise_score),
            suggested_action: c.suggested_action.clone(),
        })
        .collect()
}

#[cfg(test)]
#[path = "clean_test_support.rs"]
mod clean_test_support;

#[cfg(test)]
#[path = "clean_tests.rs"]
mod clean_tests;

#[cfg(test)]
#[path = "clean_principal_tests.rs"]
mod clean_principal_tests;

#[cfg(test)]
#[path = "clean_batch_tests.rs"]
mod clean_batch_tests;

#[cfg(test)]
#[path = "clean_discard_cascade_tests.rs"]
mod clean_discard_cascade_tests;

#[cfg(test)]
#[path = "clean_apply_suggested_tests.rs"]
mod clean_apply_suggested_tests;
