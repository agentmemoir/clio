// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the resolved retention snapshot consumed by
//! admission: the copyable thresholds, per-category offsets, version stamp, and
//! resolved mission derived from a [`crate::retention::RetentionProfile`].
//!
//! ## Owns
//! - `ResolvedRetention` and its effective-threshold accessors
//! - The curve, pack channel, provenance floor, and maintenance trigger values
//!   carried from the profile
//!
//! ## Does not own
//! - The profile object shape, validation, and mapping tables (see `retention`)
//! - Admission scoring that consumes the snapshot (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to the resolved snapshot fields or accessors should normally require
//! changes here. Changes to profile mapping numbers should not require changes here.
//!
//! Keep this module focused on the resolved retention snapshot.

use crate::retention::{
    ADMISSION_KEY_COUNT, DEFAULT_RETENTION_FLOOR, PackFidelityDefault, ProvenanceFloorDefault,
    RETENTION_PROFILE_VERSION, key_index,
};
use crate::retention_mission::ResolvedMission;
use std::sync::Arc;

/// Effective retention snapshot consumed by the admission policy. Carries the
/// resolved mission so every decision site that resolves a profile also
/// resolves the mission (empty when absent).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRetention {
    pub(crate) theta_delta: f64,
    pub(crate) novelty_floor: f64,
    pub(crate) offsets: [f64; ADMISSION_KEY_COUNT],
    pub(crate) profile_version: u32,
    pub(crate) mission: Option<Arc<ResolvedMission>>,
    pub(crate) retention_curve: bool,
    pub(crate) retention_floor: f64,
    pub(crate) pack_fidelity_default: PackFidelityDefault,
    pub(crate) provenance_floor_default: Option<ProvenanceFloorDefault>,
    pub(crate) maintenance_trigger: bool,
}

impl Default for ResolvedRetention {
    fn default() -> Self {
        Self::builtin_default()
    }
}

impl ResolvedRetention {
    /// The builtin default (`balanced` / `balanced` / zero offsets / no mission,
    /// curve on at floor `0.25`, pack channel `flow`, no provenance floor,
    /// maintenance trigger on).
    #[must_use]
    pub fn builtin_default() -> Self {
        Self {
            theta_delta: 0.0,
            novelty_floor: 0.0,
            offsets: [0.0; ADMISSION_KEY_COUNT],
            profile_version: RETENTION_PROFILE_VERSION,
            mission: None,
            retention_curve: true,
            retention_floor: DEFAULT_RETENTION_FLOOR,
            pack_fidelity_default: PackFidelityDefault::Flow,
            provenance_floor_default: None,
            maintenance_trigger: true,
        }
    }

    /// Verbosity threshold delta.
    #[must_use]
    pub const fn theta_delta(&self) -> f64 {
        self.theta_delta
    }

    /// Effective duplicate-gate novelty floor.
    #[must_use]
    pub const fn novelty_floor(&self) -> f64 {
        self.novelty_floor
    }

    /// Version stamp of the profile this snapshot resolved from.
    #[must_use]
    pub const fn profile_version(&self) -> u32 {
        self.profile_version
    }

    /// The resolved mission snapshot; `None` when the bank has no active mission.
    #[must_use]
    pub const fn mission(&self) -> Option<&Arc<ResolvedMission>> {
        self.mission.as_ref()
    }

    /// Whether the retention curve shapes ranked results.
    #[must_use]
    pub const fn retention_curve(&self) -> bool {
        self.retention_curve
    }

    /// Lowest multiplier the retention curve may apply, in `(0, 1]`.
    #[must_use]
    pub const fn retention_floor(&self) -> f64 {
        self.retention_floor
    }

    /// Default pack fidelity channel.
    #[must_use]
    pub const fn pack_fidelity_default(&self) -> PackFidelityDefault {
        self.pack_fidelity_default
    }

    /// Default provenance floor; `None` means no floor.
    #[must_use]
    pub const fn provenance_floor_default(&self) -> Option<ProvenanceFloorDefault> {
        self.provenance_floor_default
    }

    /// Whether the maintenance trigger is enabled.
    #[must_use]
    pub const fn maintenance_trigger(&self) -> bool {
        self.maintenance_trigger
    }

    /// Per-category threshold delta (verbosity delta plus the category offset).
    #[must_use]
    pub fn theta_delta_for(&self, key: &str) -> f64 {
        self.theta_delta + key_index(key).map_or(0.0, |i| self.offsets[i])
    }

    /// Apply the retention delta to a base threshold and clamp to `[0, 1]`.
    #[must_use]
    pub fn effective_threshold(&self, base: f64, key: &str) -> f64 {
        (base + self.theta_delta_for(key)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
#[path = "retention_resolved_tests.rs"]
mod retention_resolved_tests;
