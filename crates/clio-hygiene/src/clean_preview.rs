// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for target resolution and the zero-write
//! preview branch of the `hygiene_clean` mutator: the per-target action
//! resolution for `apply_suggested`, the settled-state probe, and the
//! preview's counters and skip reporting.
//!
//! ## Owns
//! - [`preview_outcome`]: the preview branch (counts, `skipped` list, no
//!   writes)
//! - [`target_already_settled`]: the discarded-or-archived probe shared by
//!   the preview and the confirmed path
//! - [`resolved_action`]: per-target action resolution shared with the
//!   confirmed path
//! - [`validate_target_id`]: the non-empty id check shared with the
//!   confirmed path
//!
//! ## Does not own
//! - The confirmed mutator loop and its log-row writes (`clean.rs`)
//! - Store SQL (flags, archive markers, discard columns — `clio-store`)
//!
//! ## Boundary
//!
//! Changes to preview counting, skip reporting, or suggestion resolution
//! should normally require changes here. Changes to confirmed mutations
//! should not.
//!
//! Keep this module focused on target resolution and previewing.

use crate::clean::{CleanAction, CleanArgs, CleanOutcome, CleanTarget};
use clio_types::{AmError, ErrorCode};

/// The preview branch: count the actionable targets and write nothing.
/// Items already discarded or archived have nothing left to apply, so they
/// contribute no count and no candidate action. Every skip is disclosed in
/// the outcome's `skipped` list: a `keep` suggestion (nothing to apply) and
/// a settled target (the end state is already reached) both land there, so a
/// preview reports the same skipped set a confirmed run over the same
/// request reports.
pub(crate) fn preview_outcome(args: &CleanArgs<'_>) -> Result<CleanOutcome, AmError> {
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
        // A `keep` suggestion has nothing to count: the preview skips it,
        // disclosed, the way the confirmed run does.
        let Some(action) = resolved_action(args.action, target)? else {
            outcome.skipped.push(target.item_id.clone());
            continue;
        };
        if target_already_settled(args, &target.item_id)? {
            // A settled target is skipped and disclosed, so the preview
            // matches what the confirmed run does with the same request.
            outcome.skipped.push(target.item_id.clone());
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
/// discarded, or it carries the archive marker. A clean reports nothing to
/// do for such a target, in either mode. A missing item is not settled: the
/// confirmed path surfaces it as a failure.
pub(crate) fn target_already_settled(args: &CleanArgs<'_>, item_id: &str) -> Result<bool, AmError> {
    let bank = args.ctx.bank.as_str();
    let active = args.store.get_item_identity(bank, item_id)?.is_some();
    if active {
        return args.store.item_is_archived(bank, item_id);
    }
    // A discarded row keeps its memory row; a missing one has none. Missing
    // items stay reportable: the confirmed path surfaces them as failures.
    Ok(args.store.get_memory_item(bank, item_id)?.is_some())
}

/// Resolve the per-target action (`apply_suggested` needs an audit
/// suggestion). `None` marks a target suggested as `keep`: nothing applies,
/// so the caller skips it rather than aborting the batch. Other unmapped
/// suggestions still fail closed.
pub(crate) fn resolved_action(
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

pub(crate) fn validate_target_id(item_id: &str) -> Result<(), AmError> {
    if item_id.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item_id is required for every clean target",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "clean_preview_tests.rs"]
mod clean_preview_tests;
