// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the published length limit on one `gist`
//! summary and the check that enforces it.
//!
//! ## Owns
//! - `GIST_MAX_CHARS`: the published maximum length, in Unicode scalar values
//! - `validate_gist`: the fail-closed check a producer runs before storing a
//!   gist
//!
//! ## Does not own
//! - Where a gist comes from or how it reaches an item (see `item` and the
//!   extraction pipeline in `clio-write`)
//! - Span verification of snapshot leaves, which a gist is never subject to
//!
//! ## Boundary
//!
//! Changes to how long a gist may be should normally require changes here.
//! Changes to how a gist is produced or stored should not.
//!
//! Keep this module focused on one bound and its check.

use crate::{AmError, ErrorCode};

/// Published maximum length for one `gist` summary, in Unicode scalar values.
///
/// A gist is model-generated prose, so it is never span-verified the way a
/// snapshot leaf is; the cap bounds how much unverified text one memory can
/// carry into retrieval. Deployments may lower but not raise it.
pub const GIST_MAX_CHARS: usize = 250;

/// Validate one `gist` summary: a present value must be at most
/// [`GIST_MAX_CHARS`] Unicode scalar values (never truncated).
///
/// # Errors
/// `OutOfRange` when the summary is longer than the published maximum. The
/// count is in scalar values, not bytes, so a multi-byte character costs one.
pub fn validate_gist(gist: &str) -> Result<(), AmError> {
    let chars = gist.chars().count();
    if chars > GIST_MAX_CHARS {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("gist must be at most {GIST_MAX_CHARS} characters, got {chars}"),
        ));
    }
    Ok(())
}
