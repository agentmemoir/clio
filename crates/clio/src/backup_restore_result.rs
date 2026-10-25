// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one thing: the words a restore's report uses
//! for what happened. It is the vocabulary the producers, the renderer, the
//! JSON payload, and the exit-code mapping all share, so no two of them can
//! spell the same outcome differently.
//!
//! ## Owns
//! - The result values a report carries
//! - The failed-step names a report names when a step did not complete
//!
//! ## Does not own
//! - Which result a run produces (see `backup_restore_postgres` and
//!   `backup_restore_sqlite`, the two placement halves)
//! - Which exit code a result maps to (see `backup_restore_exit`)
//! - How a result is rendered (see `backup_restore_report`)
//!
//! ## Boundary
//!
//! Changes to the words a result or a failed step is spelled with should
//! normally require changes here. Changes to when a run produces one should not.
//!
//! Keep this module focused on that vocabulary.
//
// The restore flow re-exports these, so the halves and the views keep importing
// them from where they have always come from.

/// A dry run: the plan, with nothing placed.
pub(crate) const RESULT_PLANNED: &str = "planned";
/// A restore that replaced the store and did not complete everything it
/// promised, or whose post-commit count check did not match.
pub(crate) const RESULT_PARTIAL: &str = "partial";
/// A restore that replaced the store and matched the archive's counts.
pub(crate) const RESULT_RESTORED: &str = "restored";
/// A restore whose user-data counts did not match the archive's.
pub(crate) const RESULT_COUNT_MISMATCH: &str = "count-mismatch";
/// A restore that loaded every user-data row, where only a derived table the
/// store rebuilds holds a different number of rows than the archive recorded.
/// Not a failure: the user data arrived, and a rebuild closes the difference.
pub(crate) const RESULT_DERIVED_DIFFERENT: &str = "derived-difference";

/// The step that did not complete when a restore's counts did not match.
pub(crate) const FAILED_STEP_COUNTS: &str = "post-restore count check";
/// The step that did not complete when the key store was not placed.
pub(crate) const FAILED_STEP_KEY_SWAP: &str = "key-store swap";
