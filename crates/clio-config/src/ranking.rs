// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for typed ranking/admission environment knobs and patch validation.
//!
//! ## Owns
//! - `RankingEnv` / `RankingEnvPatch` structures and defaults
//! - Weight renormalize-or-reject, coactivation, and trust-ratio range checks
//!
//! ## Does not own
//! - Running admission scoring or retrieval
//! - Persisting overlays into the config runtime
//!
//! ## Boundary
//!
//! Changes to ranking-env schema and validation should normally require changes here.
//! Changes to config tool dispatch should not require changes here.
//!
//! Keep this module focused on ranking-environment knobs and patch validation.

use crate::admission_knobs::{
    validate_coactivation, validate_recency_tau, validate_theta_overrides,
    validate_type_prior_overrides,
};
use crate::error::{AmError, ErrorCode};
use crate::weight_groups::normalize_weights;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use crate::admission_knobs::{
    CoactivationConstants, DEFAULT_RECENCY_TAU_DAYS, DEFAULT_THETA_ADMIT, KNOWN_ADMISSION_KEYS,
    default_type_prior, prior_for, threshold_for,
};
pub use crate::hygiene_knobs::{
    DEFAULT_HYGIENE_STALE_HALF_LIFE_DAYS, HygieneWeights, default_hygiene_stale_half_life,
};
pub use crate::weight_groups::{AdmissionWeights, IntentGateThresholds, RetrievalWeights};

/// Default retention half-life in days: the age at which the decaying share
/// of the floored age factor is halved.
pub const DEFAULT_RETENTION_HALF_LIFE_DAYS: f64 = 30.0;

/// Serde default for `retention.half_life_days` when a patch omits the field.
#[must_use]
pub const fn default_retention_half_life_days() -> f64 {
    DEFAULT_RETENTION_HALF_LIFE_DAYS
}

/// Retention shaping knobs: the half-life of the floored age factor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionKnobs {
    /// Age at which the decaying share of the floored age factor is halved,
    /// in days (default 30).
    #[serde(default = "default_retention_half_life_days")]
    pub half_life_days: f64,
}

impl Default for RetentionKnobs {
    fn default() -> Self {
        Self {
            half_life_days: DEFAULT_RETENTION_HALF_LIFE_DAYS,
        }
    }
}

/// Default trust ratio for facts the user stated.
pub const DEFAULT_TRUST_USER_STATED: f64 = 1.0;

/// Default trust ratio for facts a third party stated.
pub const DEFAULT_TRUST_THIRD_PARTY: f64 = 0.95;

/// Default trust ratio for facts the agent inferred.
pub const DEFAULT_TRUST_AGENT_INFERRED: f64 = 0.85;

/// Serde default for `trust.user_stated` when a patch omits the field.
#[must_use]
pub const fn default_trust_user_stated() -> f64 {
    DEFAULT_TRUST_USER_STATED
}

/// Serde default for `trust.third_party` when a patch omits the field.
#[must_use]
pub const fn default_trust_third_party() -> f64 {
    DEFAULT_TRUST_THIRD_PARTY
}

/// Serde default for `trust.agent_inferred` when a patch omits the field.
#[must_use]
pub const fn default_trust_agent_inferred() -> f64 {
    DEFAULT_TRUST_AGENT_INFERRED
}

/// Trust ratios for the trust multiplier, one per fact source.
///
/// Each of the three sources carries its own ratio. A fact is implicitly
/// `1.0` and has no field here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustRatios {
    /// Trust ratio for facts the user stated (default 1.0).
    #[serde(default = "default_trust_user_stated")]
    pub user_stated: f64,
    /// Trust ratio for facts a third party stated (default 0.95).
    #[serde(default = "default_trust_third_party")]
    pub third_party: f64,
    /// Trust ratio for facts the agent inferred (default 0.85).
    #[serde(default = "default_trust_agent_inferred")]
    pub agent_inferred: f64,
}

impl Default for TrustRatios {
    fn default() -> Self {
        Self {
            user_stated: DEFAULT_TRUST_USER_STATED,
            third_party: DEFAULT_TRUST_THIRD_PARTY,
            agent_inferred: DEFAULT_TRUST_AGENT_INFERRED,
        }
    }
}

/// Default trigger threshold for one maintenance-trigger condition.
pub const DEFAULT_MAINTENANCE_TRIGGER_THRESHOLD: u64 = 1;

/// Default minimum seconds between maintenance waves.
pub const DEFAULT_MAINTENANCE_COOLDOWN_SECONDS: u64 = 300;

/// Default maximum maintenance waves allowed in one hour.
pub const DEFAULT_MAINTENANCE_MAX_WAVES_PER_HOUR: u64 = 12;

/// Default hub budget for one maintenance wave.
pub const DEFAULT_MAINTENANCE_HUB_BUDGET_PER_WAVE: u64 = 16;

/// Serde default for the three maintenance-trigger thresholds.
#[must_use]
pub const fn default_maintenance_trigger_threshold() -> u64 {
    DEFAULT_MAINTENANCE_TRIGGER_THRESHOLD
}

/// Serde default for `maintenance_triggers.cooldown_seconds`.
#[must_use]
pub const fn default_maintenance_cooldown_seconds() -> u64 {
    DEFAULT_MAINTENANCE_COOLDOWN_SECONDS
}

/// Serde default for `maintenance_triggers.max_waves_per_hour`.
#[must_use]
pub const fn default_maintenance_max_waves_per_hour() -> u64 {
    DEFAULT_MAINTENANCE_MAX_WAVES_PER_HOUR
}

/// Serde default for `maintenance_triggers.hub_budget_per_wave`.
#[must_use]
pub const fn default_maintenance_hub_budget_per_wave() -> u64 {
    DEFAULT_MAINTENANCE_HUB_BUDGET_PER_WAVE
}

/// Maintenance-trigger knobs: when background maintenance waves may run.
///
/// The three thresholds are inclusive counts: a condition triggers a wave when
/// its count is greater than or equal to the threshold. A threshold of `0`
/// disables its one condition, so that condition never triggers a wave.
///
/// `max_waves_per_hour` and `hub_budget_per_wave` reject `0`: a wave rate of
/// zero silently switches maintenance off while reporting a rate-limit
/// reason, and a hub budget of zero still spends a wave, a cooldown and an
/// hourly slot while distilling nothing.
///
/// `cooldown_seconds` keeps `0`, which coherently means no cooldown. The three
/// thresholds keep `0` because it is the documented way to disable one
/// condition.
///
/// Beyond those two rejections, validation is the unsigned type range; the
/// patch surface rejects unknown fields on top of it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceTriggerKnobs {
    /// Dirty-ancestor count that triggers a wave (default 1).
    #[serde(default = "default_maintenance_trigger_threshold")]
    pub dirty_ancestor_threshold: u64,
    /// Eligible-hub count that triggers a wave (default 1).
    #[serde(default = "default_maintenance_trigger_threshold")]
    pub eligible_hub_threshold: u64,
    /// Stale-unit count that triggers a wave (default 1).
    #[serde(default = "default_maintenance_trigger_threshold")]
    pub stale_unit_threshold: u64,
    /// Minimum seconds between two maintenance waves (default 300).
    #[serde(default = "default_maintenance_cooldown_seconds")]
    pub cooldown_seconds: u64,
    /// Maximum maintenance waves allowed in one hour (default 12).
    #[serde(default = "default_maintenance_max_waves_per_hour")]
    pub max_waves_per_hour: u64,
    /// Maximum hubs one wave may distill (default 16).
    #[serde(default = "default_maintenance_hub_budget_per_wave")]
    pub hub_budget_per_wave: u64,
}

impl Default for MaintenanceTriggerKnobs {
    fn default() -> Self {
        Self {
            dirty_ancestor_threshold: DEFAULT_MAINTENANCE_TRIGGER_THRESHOLD,
            eligible_hub_threshold: DEFAULT_MAINTENANCE_TRIGGER_THRESHOLD,
            stale_unit_threshold: DEFAULT_MAINTENANCE_TRIGGER_THRESHOLD,
            cooldown_seconds: DEFAULT_MAINTENANCE_COOLDOWN_SECONDS,
            max_waves_per_hour: DEFAULT_MAINTENANCE_MAX_WAVES_PER_HOUR,
            hub_budget_per_wave: DEFAULT_MAINTENANCE_HUB_BUDGET_PER_WAVE,
        }
    }
}

/// Full ranking-environment surface returned by `ranking_env_get`.
///
/// The env is only built in code (`system_defaults`, patch application) and
/// serialized out; JSON input goes through `RankingEnvPatch`.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct RankingEnv {
    pub retrieval: RetrievalWeights,
    pub admission: AdmissionWeights,
    pub coactivation: CoactivationConstants,
    pub intent_gate: IntentGateThresholds,
    /// Admission threshold `θ_admit` default (per-key overrides win).
    pub theta_admit: f64,
    /// Per-key `θ_admit` overrides keyed by category or episodic type.
    pub theta_overrides: HashMap<String, f64>,
    /// Per-key content-type prior overrides keyed by category or episodic type.
    pub type_prior_overrides: HashMap<String, f64>,
    /// Admission recency decay constant `τ` in days. Admission scoring reads
    /// it as its age input; retrieval shaping does not. Every shipped signal
    /// source reports no prior age, so it changes no write score today: the
    /// admission recency factor is a constant `1.0`. The knob stays live for
    /// the arithmetic and for a host that supplies an explicit age.
    pub recency_tau_days: f64,
    /// Hygiene noise-score weights (appendix defaults).
    pub hygiene: HygieneWeights,
    /// Staleness half-life for hygiene scoring, in days.
    pub hygiene_stale_half_life_days: f64,
    /// Retention shaping knobs (floored age-factor half-life).
    pub retention: RetentionKnobs,
    /// Trust ratios used by the trust multiplier (fact source weights).
    pub trust: TrustRatios,
    /// Maintenance-trigger knobs: wave thresholds and rate limits.
    pub maintenance_triggers: MaintenanceTriggerKnobs,
}

impl RankingEnv {
    #[must_use]
    pub fn system_defaults() -> Self {
        Self {
            retrieval: RetrievalWeights::default(),
            admission: AdmissionWeights::default(),
            coactivation: CoactivationConstants::default(),
            intent_gate: IntentGateThresholds::default(),
            theta_admit: DEFAULT_THETA_ADMIT,
            theta_overrides: HashMap::new(),
            type_prior_overrides: HashMap::new(),
            recency_tau_days: DEFAULT_RECENCY_TAU_DAYS,
            hygiene: HygieneWeights::default(),
            hygiene_stale_half_life_days: DEFAULT_HYGIENE_STALE_HALF_LIFE_DAYS,
            retention: RetentionKnobs::default(),
            trust: TrustRatios::default(),
            maintenance_triggers: MaintenanceTriggerKnobs::default(),
        }
    }
}

/// Patch accepted by `ranking_env_set`. All fields optional.
///
/// Unknown fields are rejected so a `bypass`-style flag can never slip in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankingEnvPatch {
    pub retrieval: Option<RetrievalWeights>,
    pub admission: Option<AdmissionWeights>,
    pub coactivation: Option<CoactivationConstants>,
    pub intent_gate: Option<IntentGateThresholds>,
    pub theta_admit: Option<f64>,
    pub theta_overrides: Option<HashMap<String, f64>>,
    pub type_prior_overrides: Option<HashMap<String, f64>>,
    pub recency_tau_days: Option<f64>,
    pub hygiene: Option<HygieneWeights>,
    pub hygiene_stale_half_life_days: Option<f64>,
    pub retention: Option<RetentionKnobs>,
    pub trust: Option<TrustRatios>,
    pub maintenance_triggers: Option<MaintenanceTriggerKnobs>,
}

fn apply_retrieval(w: &RetrievalWeights) -> Result<RetrievalWeights, AmError> {
    let n = normalize_weights(&w.as_slice())?;
    Ok(RetrievalWeights::from_slice([n[0], n[1], n[2]]))
}

fn apply_admission(w: &AdmissionWeights) -> Result<AdmissionWeights, AmError> {
    let n = normalize_weights(&w.as_slice())?;
    Ok(AdmissionWeights::from_slice([n[0], n[1], n[2], n[3], n[4]]))
}

fn apply_hygiene(w: &HygieneWeights) -> Result<HygieneWeights, AmError> {
    let n = normalize_weights(&w.as_slice())?;
    Ok(HygieneWeights::from_slice([n[0], n[1], n[2], n[3], n[4]]))
}

/// Check the maintenance-trigger knobs whose `0` is not a usable value.
///
/// The two rate knobs reject `0`; the three thresholds and the cooldown accept
/// it, because each of those four has a coherent meaning at `0`.
///
/// # Errors
/// `OutOfRange` when either rate knob is `0`.
fn validate_maintenance_triggers(k: &MaintenanceTriggerKnobs) -> Result<(), AmError> {
    if k.max_waves_per_hour == 0 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "maintenance_triggers.max_waves_per_hour must be > 0",
        ));
    }
    if k.hub_budget_per_wave == 0 {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "maintenance_triggers.hub_budget_per_wave must be > 0",
        ));
    }
    Ok(())
}

/// Merge `patch` onto `base`, normalizing weight groups. Does not mutate `base`.
///
/// # Errors
/// Returns validation errors without applying partial changes.
pub fn preview_patch(base: &RankingEnv, patch: &RankingEnvPatch) -> Result<RankingEnv, AmError> {
    let mut next = base.clone();
    if let Some(r) = &patch.retrieval {
        next.retrieval = apply_retrieval(r)?;
    }
    if let Some(a) = &patch.admission {
        next.admission = apply_admission(a)?;
    }
    if let Some(c) = patch.coactivation.clone() {
        validate_coactivation(&c)?;
        next.coactivation = c;
    }
    if let Some(i) = patch.intent_gate.clone() {
        if let Some(sc) = i.skip_confidence
            && !(0.0..=1.0).contains(&sc)
        {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "intent_gate.skip_confidence must be in [0, 1]",
            ));
        }
        next.intent_gate = i;
    }
    if let Some(t) = patch.theta_admit {
        if !(0.0..=1.0).contains(&t) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "theta_admit must be in [0, 1]",
            ));
        }
        next.theta_admit = t;
    }
    if let Some(map) = patch.theta_overrides.clone() {
        validate_theta_overrides(&map)?;
        next.theta_overrides = map;
    }
    if let Some(map) = patch.type_prior_overrides.clone() {
        validate_type_prior_overrides(&map)?;
        next.type_prior_overrides = map;
    }
    if let Some(tau) = patch.recency_tau_days {
        validate_recency_tau(tau)?;
        next.recency_tau_days = tau;
    }
    if let Some(h) = &patch.hygiene {
        next.hygiene = apply_hygiene(h)?;
    }
    if let Some(half_life) = patch.hygiene_stale_half_life_days {
        if !(half_life.is_finite() && half_life > 0.0) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "hygiene_stale_half_life_days must be finite and > 0",
            ));
        }
        next.hygiene_stale_half_life_days = half_life;
    }
    if let Some(r) = &patch.retention {
        if !(r.half_life_days.is_finite() && r.half_life_days > 0.0) {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                "retention.half_life_days must be finite and > 0",
            ));
        }
        next.retention = *r;
    }
    if let Some(t) = &patch.trust {
        for (name, ratio) in [
            ("user_stated", t.user_stated),
            ("third_party", t.third_party),
            ("agent_inferred", t.agent_inferred),
        ] {
            if !(ratio.is_finite() && ratio > 0.0 && ratio <= 1.0) {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("trust ratio {name} must be finite and in (0, 1]"),
                ));
            }
        }
        next.trust = *t;
    }
    if let Some(t) = patch.maintenance_triggers {
        validate_maintenance_triggers(&t)?;
        next.maintenance_triggers = t;
    }
    Ok(next)
}

#[cfg(test)]
#[path = "ranking_tests.rs"]
mod ranking_tests;

#[cfg(test)]
#[path = "ranking_trust_tests.rs"]
mod ranking_trust_tests;

#[cfg(test)]
#[path = "ranking_maintenance_tests.rs"]
mod ranking_maintenance_tests;
