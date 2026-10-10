// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the continuous-attribute repository contract
//! Attribute `update_rule` declarations, EMA observations, and
//! ordered series reads for both backends.
//!
//! ## Owns
//! - The `ContinuousStore` trait and its call signatures
//!
//! ## Does not own
//! - Backend SQL (see the per-backend attribute modules)
//! - EMA math or admission scoring (`clio-write` / `clio-admission` own those)
//!
//! ## Boundary
//!
//! Changes to the continuous persistence surface should normally require
//! changes here. Changes to EMA math or backend SQL should not require
//! changes here.
//!
//! Keep this module focused on the continuous-attribute persistence contract.

use clio_types::{AmError, AttributeSchema, ContinuousPoint, ContinuousState};

/// Repository contract for the continuous-attribute surface.
///
/// A separate trait from [`crate::store::Store`] so each backend implements the
/// continuous surface in its attribute module without splitting the main
/// `Store` impl.
pub trait ContinuousStore: Send + Sync {
    /// Register / refresh an attribute `update_rule` declaration.
    ///
    /// The declaration is immutable per key; knobs (α/N/bounds/clamp) may be updated.
    ///
    /// # Errors
    /// Validation, immutability rejection, or storage failures.
    fn put_attribute_schema(&self, schema: &AttributeSchema, actor: &str) -> Result<(), AmError>;

    /// Read the `update_rule` declaration for an attribute key (bank-scoped).
    ///
    /// # Errors
    /// Storage or mapping failures. `Ok(None)` = undeclared in this bank.
    fn get_attribute_schema(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<AttributeSchema>, AmError>;

    /// Apply one continuous observation (transactional RMW, serialized per key).
    ///
    /// # Errors
    /// Undeclared/mismatched rule, invalid value, or storage failures.
    fn observe_continuous(
        &self,
        bank_id: &str,
        attr_key: &str,
        value: f64,
        as_of: &str,
        actor: &str,
    ) -> Result<ContinuousState, AmError>;

    /// Apply one continuous observation carrying a correction reason.
    ///
    /// Same EMA math and locking as [`ContinuousStore::observe_continuous`];
    /// the `reason` is stored on the audit row of the same transaction so a
    /// continuous correction stays attributable end to end.
    ///
    /// # Errors
    /// Undeclared/mismatched rule, invalid value, or storage failures.
    fn observe_continuous_with_reason(
        &self,
        bank_id: &str,
        attr_key: &str,
        value: f64,
        as_of: &str,
        actor: &str,
        reason: Option<&str>,
    ) -> Result<ContinuousState, AmError>;

    /// Ordered `(as_of, state, trend)` points; `as_of` filters `point.as_of <= as_of`.
    ///
    /// Context is attached per point, not cloned across the read: a
    /// point carries the key's stored context only when the point's own `as_of`
    /// is at or after the context's effective time (the preference row's
    /// `context_as_of`, falling back to the row's `as_of` on a legacy row), so
    /// points before the context was introduced carry `None`. The stored
    /// context is opened tolerantly (`None` when the key has none or the sealed
    /// value is unreadable). The store keeps one context per key, so a point
    /// carries the context effective from its introduction onward, not a
    /// per-observation context history. The `as_of` filter itself is unchanged.
    ///
    /// # Errors
    /// Storage failures.
    fn continuous_series(
        &self,
        bank_id: &str,
        attr_key: &str,
        as_of: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ContinuousPoint>, AmError>;
}
