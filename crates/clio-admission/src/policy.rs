// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for resolving admission policy from the ranking env.
//!
//! ## Owns
//! - Weight vector, per-key thresholds, priors, and recency `τ` resolution
//!
//! ## Does not own
//! - Factor signal math or admit/reject decisions
//!
//! ## Boundary
//!
//! Changes to policy resolution should normally require changes here.
//! Changes to factor formulas should not require changes here.
//!
//! Keep this module focused on policy resolution.

use clio_config::ranking::RankingEnv;
use clio_config::retention::ResolvedRetention;
use clio_config::retention_mission::MissionEvaluation;
use clio_config::{prior_for, threshold_for};

/// Admission policy resolved from one ranking-env snapshot plus the effective
/// retention profile for the bank being written.
#[derive(Debug, Clone)]
pub struct AdmissionPolicy<'a> {
    env: &'a RankingEnv,
    retention: ResolvedRetention,
    force: bool,
}

impl<'a> AdmissionPolicy<'a> {
    /// Wrap a ranking-env snapshot with the builtin retention profile.
    ///
    /// Callers that resolved a bank profile use [`Self::with_retention`].
    #[must_use]
    pub fn new(env: &'a RankingEnv) -> Self {
        Self {
            env,
            retention: ResolvedRetention::builtin_default(),
            force: false,
        }
    }

    /// Wrap a ranking-env snapshot plus a resolved per-bank retention snapshot.
    #[must_use]
    pub fn with_retention(env: &'a RankingEnv, retention: ResolvedRetention) -> Self {
        Self {
            env,
            retention,
            force: false,
        }
    }

    /// Set the caller's explicit force flag.
    ///
    /// Force waives the unconditional exact-duplicate rejection only. Every
    /// other gate — the category gate, span verification, the mission veto, the
    /// novelty floor, and the score threshold — still runs, so a forced write
    /// can never skip validation.
    #[must_use]
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Whether the caller explicitly forced this write past the
    /// exact-duplicate rejection.
    #[must_use]
    pub const fn force(&self) -> bool {
        self.force
    }

    /// Four-factor weights `w1…w4` in score order.
    #[must_use]
    pub fn weights(&self) -> [f64; 4] {
        let a = &self.env.admission;
        [a.w1, a.w2, a.w3, a.w4]
    }

    /// Effective `θ_admit` for a category or episodic-type key.
    ///
    /// `θ_effective = θ_base + verbosity_delta + theta_offsets[key]`, clamped to `[0, 1]`.
    #[must_use]
    pub fn threshold_for(&self, key: &str) -> f64 {
        let base = threshold_for(&self.env.theta_overrides, self.env.theta_admit, key);
        self.retention.effective_threshold(base, key)
    }

    /// Effective content-type prior for a category or episodic-type key.
    #[must_use]
    pub fn prior_for(&self, key: &str) -> f64 {
        prior_for(&self.env.type_prior_overrides, key)
    }

    /// Effective duplicate-gate novelty floor for this profile.
    #[must_use]
    pub const fn novelty_floor(&self) -> f64 {
        self.retention.novelty_floor()
    }

    /// Version stamp of the resolved retention profile (audit attribution).
    #[must_use]
    pub const fn profile_version(&self) -> u32 {
        self.retention.profile_version()
    }

    /// Deterministic mission evaluation over the candidate text; empty (no
    /// influence) when the bank resolves no mission. Drop hits veto outright
    /// when the entry declares reject; keep hits add a utility bonus.
    #[must_use]
    pub fn mission_evaluate(&self, text: &str) -> MissionEvaluation {
        self.retention
            .mission()
            .map_or_else(MissionEvaluation::default, |mission| mission.evaluate(text))
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod policy_tests;
