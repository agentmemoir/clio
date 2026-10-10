// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the bank-scoped retention profile tool handlers on `Runtime`.
//!
//! ## Owns
//! - `retention_profile_get` / `retention_profile_set` resolution and persistence
//! - The durable retention store path override and reload
//!
//! ## Does not own
//! - The profile object, mapping numbers, or storage format (see `crate::retention`)
//! - Admission scoring that consumes the resolved snapshot (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to retention tool semantics on Runtime should normally require changes here.
//! Changes to profile fields or storage format should not require changes here.
//!
//! Keep this module focused on retention profile Runtime handlers.

use super::{RetentionMissionSummary, RetentionProfileGetData, RetentionProfileSetData, Runtime};
use crate::error::{AmError, ErrorCode, ToolResult};
use crate::log;
use crate::retention::RetentionProfile;
use crate::retention_store::RetentionStore;
use std::path::PathBuf;

impl Runtime {
    /// Override the durable retention store path (tests / embedding).
    ///
    /// An unreadable or corrupt store file degrades to the builtin-only store.
    pub fn set_retention_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        self.retention = RetentionStore::load_lossy(&path);
        self.retention_path = Some(path);
    }

    /// Drop the durable retention path so profile writes stay in-memory only.
    pub fn clear_retention_path(&mut self) {
        self.retention_path = None;
    }

    /// Reload the retention store from [`Self::set_retention_path`] / env.
    pub fn reload_retention(&mut self) {
        if let Some(path) = self.retention_path.clone() {
            self.retention = RetentionStore::load_lossy(&path);
        }
    }

    /// Effective profile for one bank, with its source and the inert fields.
    ///
    /// # Errors
    /// An empty bank name.
    #[must_use]
    pub fn retention_profile_get(&self, bank: &str) -> ToolResult<RetentionProfileGetData> {
        ToolResult::from_result(self.retention_profile_get_inner(bank))
    }

    fn retention_profile_get_inner(&self, bank: &str) -> Result<RetentionProfileGetData, AmError> {
        if bank.trim().is_empty() {
            return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
        }
        let (profile, source) = self.retention.effective(bank);
        Ok(RetentionProfileGetData {
            bank: bank.to_owned(),
            source: source.as_str(),
            verbosity: profile.verbosity.as_str(),
            theta_offsets: profile.theta_offsets.clone(),
            duplicate_tolerance_write: profile.duplicate_tolerance_write.as_str(),
            version: profile.version,
            mission: RetentionMissionSummary::from_profile(&profile),
            accepted_but_inert: profile.accepted_but_inert(),
        })
    }

    /// Store a validated bank override (persisted when a store path is configured).
    ///
    /// # Errors
    /// An invalid profile, an empty bank, or a persistence failure.
    pub fn retention_profile_set(
        &mut self,
        bank: &str,
        profile: RetentionProfile,
    ) -> ToolResult<RetentionProfileSetData> {
        ToolResult::from_result(self.retention_profile_set_inner(bank, profile))
    }

    fn retention_profile_set_inner(
        &mut self,
        bank: &str,
        profile: RetentionProfile,
    ) -> Result<RetentionProfileSetData, AmError> {
        let previous = self.retention.bank_override(bank).cloned();
        self.retention.set_bank(bank, profile.clone())?;
        let persisted = match self.persist_retention() {
            Ok(persisted) => persisted,
            Err(e) => {
                restore_bank_override(&mut self.retention, bank, previous);
                return Err(e);
            }
        };
        log::info(
            "retention_profile_set",
            &[("bank", bank), ("scope", "bank")],
        );
        Ok(RetentionProfileSetData {
            bank: Some(bank.to_owned()),
            scope: "bank",
            accepted_but_inert: profile.accepted_but_inert(),
            profile,
            persisted,
        })
    }

    /// Store a validated deployment default applied to banks without an override.
    ///
    /// # Errors
    /// An invalid profile or a persistence failure.
    pub fn retention_profile_set_deployment(
        &mut self,
        profile: RetentionProfile,
    ) -> ToolResult<RetentionProfileSetData> {
        ToolResult::from_result(self.retention_profile_set_deployment_inner(profile))
    }

    fn retention_profile_set_deployment_inner(
        &mut self,
        profile: RetentionProfile,
    ) -> Result<RetentionProfileSetData, AmError> {
        let previous = self.retention.deployment.clone();
        self.retention.set_deployment(profile.clone())?;
        let persisted = match self.persist_retention() {
            Ok(persisted) => persisted,
            Err(e) => {
                self.retention.deployment = previous;
                return Err(e);
            }
        };
        log::info(
            "retention_profile_set",
            &[("bank", "deployment"), ("scope", "deployment")],
        );
        Ok(RetentionProfileSetData {
            bank: None,
            scope: "deployment",
            accepted_but_inert: profile.accepted_but_inert(),
            profile,
            persisted,
        })
    }

    /// Write the store to disk when a path is configured; `Ok(false)` when in-memory only.
    fn persist_retention(&self) -> Result<bool, AmError> {
        match &self.retention_path {
            Some(path) => {
                self.retention.save(path)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

/// Roll the in-memory bank override back so a failed persist leaves no change.
fn restore_bank_override(
    store: &mut RetentionStore,
    bank: &str,
    previous: Option<RetentionProfile>,
) {
    match previous {
        Some(profile) => store
            .set_bank(bank, profile)
            .expect("rollback profile was validated"),
        None => {
            store.clear_bank(bank);
        }
    }
}

#[cfg(test)]
#[path = "retention_ops_retention_tests.rs"]
mod retention_ops_retention_tests;
