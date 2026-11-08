// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the version text a release tag carries: the
//! optional leading `v`, the three numeric parts of `MAJOR.MINOR.PATCH`, and the
//! ordering that compares an installed version with a published one.
//!
//! ## Owns
//! - `Version`, its three numeric parts, and the ordering derived from them
//! - `Version::parse`, the strict reader that refuses anything else
//! - The normalized rendering: the three numbers joined by dots, no leading `v`
//!
//! ## Does not own
//! - Reading the release metadata the tag comes from (`version_check_release`)
//! - Reading the installed marker's recorded version (`version_check`)
//! - Deciding what a comparison means for the command's exit code
//!   (`version_check`)
//!
//! ## Boundary
//!
//! Changes to which version text is accepted, or to how two of them are ordered,
//! should normally require changes here. Changes to what a release answer means
//! should not.
//!
//! The reader is deliberately strict: no pre-release suffix, no build metadata,
//! no two-part or four-part form, and no whitespace. A tag this reader refuses is
//! an invalid release rather than a version guessed at, because a guess would
//! compare an installed version with something that is not one.
//!
//! Keep this module focused on the version text and its ordering.

use std::fmt;

/// One `MAJOR.MINOR.PATCH` version, ordered by its parts in that order.
///
/// The field order is the ordering: a tuple of the three numbers compares
/// exactly as a version does, so no hand-written comparison can drift from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    /// The version `text` states, or `None` when it is not exactly
    /// `[v]MAJOR.MINOR.PATCH` with each part made of digits.
    ///
    /// A leading `v` is what a release tag carries, so it is accepted and never
    /// rendered back. Every other character is part of the number or refused:
    /// `1.2.3-beta` and `1.2.3+build` are not versions this command compares.
    pub(crate) fn parse(text: &str) -> Option<Version> {
        let digits = text
            .strip_prefix('v')
            .or_else(|| text.strip_prefix('V'))
            .unwrap_or(text);
        let mut parts = digits.split('.');
        let major = number(parts.next()?)?;
        let minor = number(parts.next()?)?;
        let patch = number(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

/// One part of a version, refused when it is empty, holds a non-digit, or does
/// not fit the number type.
fn number(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

impl fmt::Display for Version {
    /// The normalized form: the three numbers joined by dots, with no `v`, and
    /// with the value each part parses to rather than the digits it arrived as.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
#[path = "version_check_semver_tests.rs"]
mod tests;
