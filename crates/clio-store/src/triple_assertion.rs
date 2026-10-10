// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the question the write layer asks before it refuses
//! a triple end: whether a bank ever asserted a subject, predicate, and object.
//!
//! ## Owns
//! - The `TripleAssertion` supertrait of `Store` and its default answer
//!
//! ## Does not own
//! - The lookup on SQLite and Postgres (see `sqlite_triple_asserted` and
//!   `postgres_triple_asserted`)
//! - The refusal of an unknown triple end (see `clio-write`)
//!
//! ## Boundary
//!
//! Changes to the question should normally require changes here and in the two
//! backend files. Changes to how triples close should not require changes here.
//!
//! Keep this module focused on the triple assertion question.

use clio_types::AmError;

/// Whether a bank ever asserted a triple, open or ended. Replication does not ask
/// this question: a replica that has not yet received a triple still applies a
/// close as a no-op.
pub trait TripleAssertion {
    /// Whether the bank ever asserted this subject, predicate, and optional object,
    /// open or ended. With no object given, any row for the pair counts.
    ///
    /// A backend that does not override this keeps the default, which reports the
    /// triple as asserted, so such a backend keeps its old behavior.
    ///
    /// # Errors
    /// Storage failures.
    fn triple_asserted_ever(
        &self,
        _bank_id: &str,
        _subject: &str,
        _predicate: &str,
        _object: Option<&str>,
    ) -> Result<bool, AmError> {
        Ok(true)
    }
}

#[cfg(test)]
#[path = "triple_assertion_tests.rs"]
mod tests;
