// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the label space of the PostgreSQL advisory
//! locks this crate takes: one named constant per lock, in one place.
//!
//! ## Owns
//! - The value of every advisory-lock key the crate uses
//!
//! ## Does not own
//! - Taking or releasing a lock (`pg_lock`, `postgres`, `bank_registration`)
//! - Anything the locks are held around
//!
//! ## Boundary
//!
//! A new advisory lock means a new constant here, never a literal at the call
//! site. Changes to what a lock protects should not require changes here.
//!
//! Keep this module focused on naming the keys and proving they differ.

/// Serializes the shared-Postgres tests, so two test processes never run
/// against one database at once (`"clio_tst"`).
pub(crate) const TEST_SERIALIZATION: i64 = 0x636C_696F_5F74_7374;

/// Serializes the store bootstrap's schema and trigger DDL (`"clio_bst"`).
pub(crate) const STORE_BOOTSTRAP: i64 = 0x636C_696F_5F62_7374;

#[cfg(test)]
mod tests {
    use super::{STORE_BOOTSTRAP, TEST_SERIALIZATION};

    /// Two subsystems that took the same key would release each other's lock:
    /// the second release succeeds, so the first subsystem's hold silently ends.
    #[test]
    fn the_advisory_keys_are_distinct() {
        assert_ne!(
            TEST_SERIALIZATION, STORE_BOOTSTRAP,
            "two subsystems share an advisory-lock key"
        );
    }
}
