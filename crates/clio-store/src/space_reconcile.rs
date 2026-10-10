// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reconciling a concrete store's recorded
//! embedding space with the effective configuration at open time.
//!
//! ## Owns
//! - `SpaceReconcile`: the open-time space-reconciliation seam for both backends
//! - `configured_embedding_space` / `space_switch_confirmed`: effective-config readers
//!
//! ## Does not own
//! - The space record itself or the switch mechanics (`embedding_space` / backends)
//! - Effective-config merging or env overlays (`clio-config`)
//!
//! ## Boundary
//!
//! Changes to how open-time reconciliation is triggered should normally require
//! changes here. Changes to switch mechanics or config merging should not.
//!
//! Keep this module focused on open-time space reconciliation.

use crate::embedding_space::{EmbeddingSpace, SpaceSwitchReport};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use clio_types::AmError;
use serde_json::Value;

/// A store whose recorded embedding space can be reconciled with configuration.
pub trait SpaceReconcile {
    /// Apply the configured space (a no-op when it matches or is `None`).
    ///
    /// # Errors
    /// Refused confirmation for a destructive switch, or storage/DDL failures.
    fn reconcile_space(
        &mut self,
        space: Option<&EmbeddingSpace>,
        confirm: bool,
    ) -> Result<Option<SpaceSwitchReport>, AmError>;
}

impl SpaceReconcile for SqliteStore {
    fn reconcile_space(
        &mut self,
        space: Option<&EmbeddingSpace>,
        confirm: bool,
    ) -> Result<Option<SpaceSwitchReport>, AmError> {
        match space {
            Some(space) => self.apply_configured_space(space, confirm).map(Some),
            None => Ok(None),
        }
    }
}

impl SpaceReconcile for PostgresStore {
    fn reconcile_space(
        &mut self,
        space: Option<&EmbeddingSpace>,
        confirm: bool,
    ) -> Result<Option<SpaceSwitchReport>, AmError> {
        match space {
            Some(space) => self.apply_configured_space(space, confirm).map(Some),
            None => Ok(None),
        }
    }
}

/// Resolve the configured embedding space from an effective config document.
///
/// # Errors
/// A half-configured or unparsable `embed.model`/`embed.dims` pair.
pub fn configured_embedding_space(effective: &Value) -> Result<Option<EmbeddingSpace>, AmError> {
    let model = effective.pointer("/embed/model").and_then(Value::as_str);
    let dims = effective.pointer("/embed/dims").and_then(|v| match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    });
    EmbeddingSpace::from_configured(model, dims.as_deref())
}

/// True when the effective config explicitly opts into a destructive space switch.
#[must_use]
pub fn space_switch_confirmed(effective: &Value) -> bool {
    effective
        .pointer("/embed/switch_space")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
