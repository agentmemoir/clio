// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tolerant persona-preference context
//! lookup that the SQLite continuous-attribute series read layers onto its
//! points.
//!
//! ## Owns
//! - `SqliteStore::preference_stored_context` (open the stored preference
//!   context for one attribute key, degrading to `None`)
//!
//! ## Does not own
//! - Attribute schema and continuous-state persistence (`sqlite_attribute`)
//! - Continuous-series formulas (`attribute_map` / `clio-types`)
//! - Persona preference writes (`sqlite_persona`, `persona_store`)
//!
//! ## Boundary
//!
//! Changes to the preference-context read should normally require changes
//! here. Changes to the continuous-series SQL should not require changes here.
//!
//! Keep this module focused on the preference-context lookup for continuous series.

use crate::migrate::IntoAmError;
use crate::persona_store::open_persona_context;
use crate::sqlite::SqliteStore;
use clio_types::AmError;
use rusqlite::{OptionalExtension, params};

impl SqliteStore {
    /// Open the stored persona-preference context for one key, tolerantly:
    /// no row, no sealed value, or an unreadable body all yield `None` so the
    /// series read keeps working after erasure (same policy as persona reads).
    /// Returns the opened text plus its effective time (`context_as_of`, or the
    /// row's `as_of` for a legacy row without the column).
    pub(crate) fn preference_stored_context(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<(String, String)>, AmError> {
        let row = self.with_conn(|conn| {
            conn.query_row(
                "SELECT context_cipher, context_as_of, as_of FROM persona_preferences
                 WHERE bank_id = ?1 AND attr_key = ?2",
                params![bank_id, attr_key],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .am("preference context read")
        })?;
        let Some((cipher, context_as_of, row_as_of)) = row else {
            return Ok(None);
        };
        let Some(text) = open_persona_context(self.kms().as_ref(), cipher.as_deref()) else {
            return Ok(None);
        };
        Ok(Some((text, context_as_of.unwrap_or(row_as_of))))
    }
}

#[cfg(test)]
#[path = "sqlite_attribute_context_tests.rs"]
mod sqlite_attribute_context_tests;
