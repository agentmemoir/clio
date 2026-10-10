// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one fact about a deployment that decides
//! whether multi-host sync is in play: whether the effective config omits sync.
//!
//! ## Owns
//! - `single_host`, the reading of the `/sync/omit` pointer and its default
//!
//! ## Does not own
//! - What the omission means for `clio sync status` (see `sync_cli`)
//! - The wizard step it gates (see `setup_steps`)
//! - The key verbs, which are deliberately not gated on it
//!
//! ## Boundary
//!
//! Changes to when a deployment counts as single-host should normally require
//! changes here. Changes to what either caller then does should not.
//!
//! Keep this module focused on the one predicate.
//!
//! # Why one definition rather than two
//!
//! The wizard and the CLI both need this answer and neither owns the config. Two
//! readers of the same pointer is two places to change when the default moves,
//! and nothing would have failed when they drifted: a deployment would be
//! single-host to one caller and multi-host to the other. The name says what the
//! pointer means, and both call sites say which side they are on.

use serde_json::Value;

/// True when the effective config omits sync.
///
/// `/sync/omit` absent, non-boolean, or explicitly `true` all read as
/// single-host. A deployment that never enabled sync therefore behaves the same
/// as one that turned it off, which is what keeps the wizard from asking a
/// question about a relay this build will not run.
#[must_use]
pub(crate) fn single_host(effective: &Value) -> bool {
    effective
        .pointer("/sync/omit")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

#[cfg(test)]
#[path = "setup_sync_config_tests.rs"]
mod tests;
