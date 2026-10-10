// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the default answer of the `TripleAssertion` supertrait: a backend
//! that does not override it reports every triple as asserted.
//!
//! ## Owns
//! - Assertions over the default `triple_asserted_ever`
//!
//! ## Does not own
//! - The SQLite and Postgres lookups (see their own test modules)
//!
//! ## Boundary
//!
//! Changes to the default answer should normally require changes here. Changes to the
//! backend lookups should not.
//!
//! Keep this module focused on the default answer.

use super::TripleAssertion;

/// A backend that keeps the default answer.
struct Keeps;

impl TripleAssertion for Keeps {}

#[test]
fn a_backend_without_an_override_reports_every_triple_as_asserted() {
    assert!(
        Keeps
            .triple_asserted_ever("bank", "nobody", "lives_in", None)
            .expect("default")
    );
    assert!(
        Keeps
            .triple_asserted_ever("bank", "nobody", "lives_in", Some("Paris"))
            .expect("default")
    );
}
