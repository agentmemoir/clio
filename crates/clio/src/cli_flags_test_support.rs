// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the one way the test suites read the reserved surfaces' flag
//! map, so a suite that is not testing the empty-value refusal does not have to
//! state that precondition nine times over.
//!
//! ## Owns
//! - `flags_of`, the flag map for an argv with no empty value in it
//! - `RESERVED_FLAGS`, the union list the shared mechanics tests parse against
//!
//! ## Does not own
//! - The refusal itself, which `cli_args_tests` and the surfaces' own tests assert
//! - The map's contents, which each suite asserts for its own surface
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds, and holds no test function:
//! two suites need it, so it cannot belong to either one.

use std::collections::HashMap;

/// Every flag any reserved surface accepts, as one union.
///
/// The mechanics tests here pair flags that belong to different surfaces, so
/// they need one list wide enough for all of them. A surface-specific test
/// passes that surface's own list to `parse_flags` instead.
pub(crate) const RESERVED_FLAGS: &[&str] = &[
    "actions",
    "backend",
    "bank",
    "bind",
    "confirm",
    "db",
    "dir",
    "dry-run",
    "insecure-no-auth",
    "out",
    "path",
    "plan",
    "profile",
    "profile-file",
    "profiles",
    "scope",
    "services",
    "sessions",
    "shared-bank",
    "target",
    "token-file",
];

/// The flag map `parse_flags` builds for `argv` against [`RESERVED_FLAGS`].
///
/// # Panics
/// When `argv` contains an empty flag value or an unknown flag. Both are a
/// refusal, not a map, so a suite that wants to see one must call `parse_flags`
/// and read the error itself — which is what `cli_args_tests` does.
pub(crate) fn flags_of(argv: &[&str]) -> HashMap<String, String> {
    crate::cli_top_level::parse_flags(argv.iter().map(|token| (*token).to_owned()), RESERVED_FLAGS)
        .expect("this argv must hold no empty flag value and no unknown flag")
}
