// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one scan every caller uses to stop
//! reading options at the `--` delimiter: the arguments before the first `--`,
//! or all of them when the list holds none.
//!
//! ## Owns
//! - `before_delimiter`, the one entry point every caller uses
//!
//! ## Does not own
//! - The argument grammar that calls it (see `cli_args`)
//!
//! ## Boundary
//!
//! Changes to what counts as the delimiter should normally require changes
//! here. Adding a command or a flag should not.
//!
//! Keep this module focused on the delimiter scan and nothing else. It is its
//! own file because the grammar is at its own size limit and this is a separate
//! concern.

/// The arguments before the first `--` delimiter, or all of them.
pub(crate) fn before_delimiter(args: &[String]) -> &[String] {
    &args[..args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len())]
}
