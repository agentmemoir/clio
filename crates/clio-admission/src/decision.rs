// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the category gate and scoring decisions.
//!
//! ## Owns
//! - Semantic whitelist and episodic allowlist enforcement
//! - `decide` / preview parity (never writes)
//!
//! ## Does not own
//! - Factor signal math (see `factors`)
//! - Gated writes and telemetry (see `gated`)
//!
//! ## Boundary
//!
//! Changes to gate order or decision shape should normally require changes here.
//! Changes to factor formulas should not require changes here.
//!
//! Keep this module focused on gate order and decisions.

use crate::factors::{
    clamp01, confidence_score, factor_unavailable, is_exact_duplicate, novelty_jaccard,
    recency_decay, utility_score, weighted_score,
};
use crate::policy::AdmissionPolicy;
use clio_types::{AmError, EpisodicType, ErrorCode, ItemKind, SemanticCategory};
use serde::{Deserialize, Serialize};

/// Machine-readable marker on a rejection caused by an identical stored item.
///
/// This rejection is unconditional: it does not depend on the retention
/// profile's novelty floor, so no configuration can switch it off. Hosts match
/// on the prefix to tell it apart from a floor rejection, which stays
/// tunable.
pub const EXACT_DUPLICATE_REASON: &str = "exact_duplicate";

/// Whether a rejection reason is the unconditional exact-duplicate rejection.
#[must_use]
pub fn is_exact_duplicate_reason(reason: &str) -> bool {
    reason.starts_with(EXACT_DUPLICATE_REASON)
}

/// A long-term write candidate: semantic (whitelisted category) or episodic (type tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Candidate {
    /// Semantic write gated by the whitelist.
    Semantic(SemanticCategory),
    /// Episodic write gated by the type-tag allowlist.
    Episodic(EpisodicType),
}

impl Candidate {
    /// Wire key for thresholds, priors, and events.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Semantic(c) => c.as_str(),
            Self::Episodic(t) => t.as_str(),
        }
    }

    /// Item kind this candidate must be stored as.
    #[must_use]
    pub const fn kind(self) -> ItemKind {
        match self {
            Self::Semantic(_) => ItemKind::Semantic,
            Self::Episodic(_) => ItemKind::Episodic,
        }
    }

    /// Parse a semantic candidate; unknown categories are an error here and a
    /// logged rejection through [`decide_raw`].
    ///
    /// # Errors
    /// Unknown category strings.
    pub fn parse_semantic(raw: &str) -> Result<Self, AmError> {
        Ok(Self::Semantic(SemanticCategory::parse(raw)?))
    }

    /// Parse an episodic candidate; unknown tags are an error here and a
    /// logged rejection through [`decide_raw`].
    ///
    /// # Errors
    /// Unknown episodic-type strings.
    pub fn parse_episodic(raw: &str) -> Result<Self, AmError> {
        Ok(Self::Episodic(EpisodicType::parse(raw)?))
    }
}

/// Raw signal inputs for one scoring run.
///
/// Hand-built inputs are for previews, tests, and explicit scoring calls.
/// Gated writes must derive these from the item through a [`SignalSource`]
/// (see `gated`), so the scored text can never detach from the persisted bytes.
#[derive(Debug, Clone, Default)]
pub struct SignalInputs {
    /// Candidate text for utility, confidence, and novelty signals.
    pub text: String,
    /// Provenance for the confidence signal.
    pub source_type: Option<clio_types::SourceType>,
    /// Neighbor texts for novelty; `None` means the store backend is down.
    pub neighbors: Option<Vec<String>>,
    /// Age of the nearest prior in days; `None` when there is no prior or the
    /// source reports no age (see `SignalSource::prior_age_days`).
    pub prior_age_days: Option<f64>,
    /// Set when the recency backend itself is down (distinct from no prior).
    pub recency_unavailable: bool,
}

/// Five scored factors in `w1…w5` order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FactorBreakdown {
    /// Future utility.
    pub utility: f64,
    /// Factual confidence.
    pub confidence: f64,
    /// Semantic novelty.
    pub novelty: f64,
    /// Temporal recency.
    pub recency: f64,
    /// Content-type prior.
    pub type_prior: f64,
}

impl FactorBreakdown {
    fn as_array(self) -> [f64; 5] {
        [
            self.utility,
            self.confidence,
            self.novelty,
            self.recency,
            self.type_prior,
        ]
    }

    fn zero() -> Self {
        Self {
            utility: 0.0,
            confidence: 0.0,
            novelty: 0.0,
            recency: 0.0,
            type_prior: 0.0,
        }
    }
}

/// Admit/reject outcome; rejections carry a machine-readable reason.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// True when the score cleared `θ_admit` and the duplicate gate.
    pub pass: bool,
    /// Weighted score in `[0, 1]` (`0.0` for gate rejections).
    pub admission_score: f64,
    /// Per-factor signals.
    pub factors: FactorBreakdown,
    /// Set when `pass` is false.
    pub rejection_reason: Option<String>,
    /// Version stamp of the retention profile this decision was scored under.
    pub profile_version: u32,
    /// Mission rule ids of the keep/drop examples that matched the candidate
    /// text (audit + preview). Empty when the bank resolves no mission.
    #[serde(default)]
    pub mission_rule_ids: Vec<String>,
    /// Human-readable labels parallel to `mission_rule_ids`.
    #[serde(default)]
    pub mission_rule_labels: Vec<String>,
}

/// Score one candidate: gate, then five factors, then threshold.
///
/// # Errors
/// `factor_unavailable` when a required signal backend is down.
pub fn decide(
    candidate: Candidate,
    signals: &SignalInputs,
    policy: &AdmissionPolicy<'_>,
) -> Result<Decision, AmError> {
    decide_ungated(candidate.key(), signals, policy)
}

/// Five-factor score for a key with **no category gate**.
///
/// Used by the belief-writes create path, where the category gate is
/// N/A but five-factor admission still MUST run on a new belief identity.
/// Still uses the same factor formulas and thresholds keyed by `key`.
///
/// An item whose token set equals a stored neighbor's is rejected as an exact
/// duplicate regardless of `novelty_floor()`, so no retention setting can
/// switch the rejection off. Only `AdmissionPolicy::with_force(true)` waives
/// it; the floor, the mission veto, and the score threshold still apply.
///
/// # Errors
/// `factor_unavailable` when a required signal backend is down.
pub fn decide_ungated(
    key: &str,
    signals: &SignalInputs,
    policy: &AdmissionPolicy<'_>,
) -> Result<Decision, AmError> {
    let neighbors = signals
        .neighbors
        .as_ref()
        .ok_or_else(|| factor_unavailable("novelty neighbor store"))?;
    if signals.recency_unavailable {
        return Err(factor_unavailable("recency store"));
    }
    // Mission influence is arithmetic only: the utility term moves; confidence,
    // novelty, recency, and the type prior never do. A keep hit cannot rescue a
    // category-gate or span-verification failure because those reject before
    // scoring ever runs.
    let mission = policy.mission_evaluate(&signals.text);
    let utility = clamp01(utility_score(&signals.text) + mission.utility_delta);
    let factors = FactorBreakdown {
        utility,
        confidence: clamp01(confidence_score(signals.source_type, &signals.text)),
        novelty: novelty_jaccard(&signals.text, neighbors),
        recency: recency_decay(signals.prior_age_days, policy.recency_tau_days()),
        type_prior: clamp01(policy.prior_for(key)),
    };
    let score = weighted_score(policy.weights(), factors.as_array());
    let threshold = policy.threshold_for(key);
    let floor = policy.novelty_floor();
    // A reject-declared drop hit wins over every keep hit and rejects the write
    // outright; the veto never applies to a gate failure because gates run first.
    if let Some(rule_id) = mission.drop_reject_id {
        return Ok(Decision {
            pass: false,
            admission_score: score,
            factors,
            rejection_reason: Some(format!("mission_drop:{rule_id}")),
            profile_version: policy.profile_version(),
            mission_rule_ids: mission.rule_ids.clone(),
            mission_rule_labels: mission.rule_labels.clone(),
        });
    }
    if is_exact_duplicate(&signals.text, neighbors) && !policy.force() {
        return Ok(Decision {
            pass: false,
            admission_score: score,
            factors,
            rejection_reason: Some(format!(
                "{EXACT_DUPLICATE_REASON}: identical text is already stored for `{key}`"
            )),
            profile_version: policy.profile_version(),
            mission_rule_ids: mission.rule_ids.clone(),
            mission_rule_labels: mission.rule_labels.clone(),
        });
    }
    if factors.novelty < floor {
        return Ok(Decision {
            pass: false,
            admission_score: score,
            factors,
            rejection_reason: Some(format!(
                "duplicate tolerance: novelty {:.3} < floor {floor:.3} for `{key}`",
                factors.novelty
            )),
            profile_version: policy.profile_version(),
            mission_rule_ids: mission.rule_ids.clone(),
            mission_rule_labels: mission.rule_labels.clone(),
        });
    }
    if score >= threshold {
        Ok(Decision {
            pass: true,
            admission_score: score,
            factors,
            rejection_reason: None,
            profile_version: policy.profile_version(),
            mission_rule_ids: mission.rule_ids.clone(),
            mission_rule_labels: mission.rule_labels.clone(),
        })
    } else {
        Ok(Decision {
            pass: false,
            admission_score: score,
            factors,
            rejection_reason: Some(format!(
                "score {score:.3} < theta {threshold:.3} for `{key}`"
            )),
            profile_version: policy.profile_version(),
            mission_rule_ids: mission.rule_ids.clone(),
            mission_rule_labels: mission.rule_labels.clone(),
        })
    }
}

pub(crate) fn gate_reject(kind: &str, key: &str, reason: &str, profile_version: u32) -> Decision {
    Decision {
        pass: false,
        admission_score: 0.0,
        factors: FactorBreakdown::zero(),
        rejection_reason: Some(format!("{kind} gate: {reason} for `{key}`")),
        profile_version,
        mission_rule_ids: Vec::new(),
        mission_rule_labels: Vec::new(),
    }
}

/// Score from raw discriminator strings; unknown categories or types become
/// logged rejections instead of parse errors.
///
/// # Errors
/// Only for structurally invalid `kind` values or unavailable factor backends.
pub fn decide_raw(
    kind: &str,
    key: &str,
    signals: &SignalInputs,
    policy: &AdmissionPolicy<'_>,
) -> Result<Decision, AmError> {
    match kind {
        "semantic" => match SemanticCategory::parse(key) {
            Ok(category) => decide(Candidate::Semantic(category), signals, policy),
            Err(_) => Ok(gate_reject(
                "category",
                key,
                "not in whitelist",
                policy.profile_version(),
            )),
        },
        "episodic" => match EpisodicType::parse(key) {
            Ok(tag) => decide(Candidate::Episodic(tag), signals, policy),
            Err(_) => Ok(gate_reject(
                "episodic_type",
                key,
                "not in allowlist",
                policy.profile_version(),
            )),
        },
        other => Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("kind must be semantic|episodic, got `{other}`"),
        )),
    }
}

/// Preview the decision for a candidate without writing.
///
/// # Errors
/// Same as [`decide`]; never writes.
pub fn admit_preview(
    candidate: Candidate,
    signals: &SignalInputs,
    policy: &AdmissionPolicy<'_>,
) -> Result<Decision, AmError> {
    decide(candidate, signals, policy)
}

/// Preview from raw discriminator strings without writing.
///
/// # Errors
/// Same as [`decide_raw`]; never writes.
pub fn admit_preview_raw(
    kind: &str,
    key: &str,
    signals: &SignalInputs,
    policy: &AdmissionPolicy<'_>,
) -> Result<Decision, AmError> {
    decide_raw(kind, key, signals, policy)
}

#[cfg(test)]
#[path = "decision_tests.rs"]
mod decision_tests;

#[cfg(test)]
#[path = "decision_mission_tests.rs"]
mod decision_mission_tests;

#[cfg(test)]
#[path = "decision_exact_duplicate_tests.rs"]
mod decision_exact_duplicate_tests;
