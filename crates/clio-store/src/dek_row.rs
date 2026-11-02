// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module builds the composite row identity that binds a DEK envelope to
//! rows whose stored id is not known at seal time.
//!
//! ## Owns
//! - `composite_row_id`, the length-prefixed encoding of ordered text fields
//!
//! ## Does not own
//! - The envelope cipher and its associated data (see `dek`)
//! - Persona or sync row SQL
//!
//! ## Boundary
//!
//! Changes to the composite encoding should normally require changes here and
//! in every row identity built from it.
//!
//! Keep this module focused on row identity encoding.

use std::fmt::Write as _;

/// Build an unambiguous composite row id from ordered text fields.
///
/// Each field is preceded by its length as eight lowercase hex digits. Persona
/// and sync rows use it to bind one envelope to the values that identify the
/// row when the stored row id is not known at seal time.
pub fn composite_row_id(fields: &[&str]) -> String {
    let mut out = String::new();
    for field in fields {
        let _ = write!(out, "{:08x}", field.len());
        out.push_str(field);
    }
    out
}

#[cfg(test)]
#[path = "dek_row_tests.rs"]
mod dek_row_tests;
