// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the hash builder behind the import planner's
//! bundle id indexes, and for counting index probes in tests.
//!
//! ## Owns
//! - `CountingBuildHasher`, the test-build hasher for the item and opaque indexes
//! - The per-thread probe count, its reset, and its reader
//!
//! ## Does not own
//! - Index construction and lookup (`import_apply.rs`)
//! - Bundle parsing and entity types (`bundle.rs`)
//!
//! ## Boundary
//!
//! Changes to how index keys are hashed in tests should normally require changes
//! here. Changes to which rows the index holds should not require changes here.
//!
//! Keep this module focused on the test hasher and its probe count.

use std::collections::hash_map::{DefaultHasher, RandomState};
use std::hash::BuildHasher;

/// Builds one hasher per index probe and counts the probe.
#[derive(Default)]
pub(super) struct CountingBuildHasher(RandomState);

impl BuildHasher for CountingBuildHasher {
    type Hasher = DefaultHasher;

    fn build_hasher(&self) -> Self::Hasher {
        PROBES.with(|probes| probes.set(probes.get() + 1));
        self.0.build_hasher()
    }
}

thread_local! {
    static PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Clears the calling thread's probe count.
pub(super) fn reset_probes() {
    PROBES.with(|probes| probes.set(0));
}

/// The calling thread's probe count since [`reset_probes`].
pub(super) fn probe_count() -> usize {
    PROBES.with(std::cell::Cell::get)
}
