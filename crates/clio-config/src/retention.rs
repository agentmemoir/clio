// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the per-bank retention profile policy object and its
//! deterministic resolution into admission thresholds and duplicate cutoffs.
//!
//! ## Owns
//! - `RetentionProfile`, its defaults, validation, and resolution into `ResolvedRetention`
//! - The re-exported level enums and mapping tables (defined in `retention_levels`)
//!
//! ## Does not own
//! - Storing profiles per bank (see `retention_store`)
//! - The resolved snapshot consumed by admission (see `retention_resolved`)
//! - Running admission scoring or applying the resolved snapshot (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to profile fields or resolution should normally require changes here.
//! Changes to level names or mapping numbers should require changes in `retention_levels`.
//! Changes to bank-scoped storage or resolution order should not require changes here.
//!
//! Keep this module focused on the retention profile object and its mapping.
//!
//! ## Mapping tables (exact numbers)
//!
//! `theta_effective = theta_base + verbosity_delta + theta_offsets[category]`, clamped to
//! `[0, 1]`. `novelty_floor_effective = max(verbosity_novelty_floor, tolerance_write_floor)`;
//! a candidate whose novelty factor is below that floor is rejected at the duplicate gate
//! even when its weighted score clears the threshold.
//!
//! | `verbosity` | `theta_delta` | `novelty_floor` |
//! |-------------|---------------|-----------------|
//! | `selective` | `+0.08` | `0.15` |
//! | `balanced` | `0.00` | `0.00` |
//! | `permissive` | `-0.08` | `0.00` |
//!
//! | `duplicate_tolerance_write` | `novelty_floor` | rejects |
//! |-----------------------------|-----------------|---------|
//! | `strict` | `0.25` | token-set similarity `>= 0.75` |
//! | `balanced` | `0.00` | nothing extra (default; unchanged behavior) |
//! | `lenient` | `0.00` | nothing extra (explicit declaration) |
//!
//! `balanced` and `lenient` share the zero write-side floor so the default profile cannot
//! change existing admission decisions.
//!
//! ## Where the write-side floor applies
//!
//! The floor is evaluated admission on every gated long-term write
//! surface, not only `store`: `store`, `canonical_put`, `triple_add`,
//! `task_upsert`, `failure_record`, `belief_observe` create, raw ingest, and
//! plaintext portability `import`. A candidate whose novelty falls below the
//! floor is rejected at the duplicate gate even when its weighted score clears
//! `theta`.
//!
//! For upsert-shaped writes this is intentional. An identical re-write of a
//! `canonical_put` value, a `triple_add` edge, or a `task_upsert` definition is
//! a redundant revision (zero change), so `strict`/`selective` reject it; a
//! changed value scores above the floor and is admitted. `failure_record` rows
//! are compared by their carrier text (lesson plus snapshot, which carries
//! `attempt_n`), so a repeated attempt with the same lesson is highly similar
//! and is likewise rejected under a non-zero floor. The default `balanced`
//! floor is `0.0`, so none of this changes default behavior.
//!
//! ## Read-time duplicate cap (independent from the write side)
//!
//! `duplicate_tolerance_read` drives a separate read-time page cap with its own token-set
//! Jaccard thresholds. It never shares a value, default, or rationale with
//! `duplicate_tolerance_write`.
//!
//! | `duplicate_tolerance_read` | `similarity_cap` | effect |
//! |----------------------------|------------------|--------|
//! | `strict` | `0.80` | suppress any page item whose similarity to a higher-ranked item is `>= 0.80` |
//! | `balanced` | `0.90` | same cap at `0.90`; neutral default, applied only when a dedup mode is active |
//! | `lenient` | `0.95` | same cap at `0.95` |
//!
//! A retrieval with no scope preset, no `prefer_consolidated`, and the neutral `balanced`
//! value applies no read-time cap, so pre-existing recall output is unchanged.
//!
//! ## Version history and the two surfaces
//!
//! Version 2 added the optional `mission` field: a deterministic keep/drop policy
//! (see `retention_mission`). Version 3 added five defaulted fields: the retention
//! curve flag and floor, the pack fidelity channel default, the optional provenance
//! floor default, and the maintenance trigger flag.
//!
//! No field changed meaning across those versions, so an older stored profile reads
//! the same values it always did with the newer fields defaulted. The shared reader
//! ([`RetentionProfile::validate`]) accepts only the current version, and a request
//! that omits `version` reads the current default. A stored file still carrying a
//! version 1 or 2 marker is converted to the current version before it is typed (see
//! `retention_store`), so only a request can carry an older marker.

use crate::admission_knobs::KNOWN_ADMISSION_KEYS;
use crate::error::{AmError, ErrorCode};
use crate::retention_mission::{ResolvedMission, RetentionMission};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub use crate::retention_levels::{
    BALANCED_READ_SIMILARITY, DuplicateTolerance, LENIENT_READ_SIMILARITY, PERMISSIVE_THETA_DELTA,
    PackFidelityDefault, ProvenanceFloorDefault, RecallScope, SELECTIVE_NOVELTY_FLOOR,
    SELECTIVE_THETA_DELTA, STRICT_READ_SIMILARITY, STRICT_WRITE_NOVELTY_FLOOR, Verbosity,
};
pub use crate::retention_resolved::ResolvedRetention;

/// Version stamp of the retention profile object. Version 2 added the optional
/// `mission` field; version 3 adds the five defaulted retention fields. A stored
/// file carrying an older marker is converted to this version before it is typed;
/// a request must carry this version, and reads it as the default when omitted.
pub const RETENTION_PROFILE_VERSION: u32 = 3;

/// Largest absolute per-category theta offset a profile may carry.
pub const MAX_THETA_OFFSET: f64 = 0.20;

/// Default retention floor: the lowest multiplier the retention curve may apply.
pub const DEFAULT_RETENTION_FLOOR: f64 = 0.25;

/// Number of admission keys a resolved profile indexes.
pub const ADMISSION_KEY_COUNT: usize = 11;

/// Capability that first honors the inert profile fields.
pub const INERT_FIELDS_HONORED_BY: &str = "recall_scope_and_dedup";

const fn default_true() -> bool {
    true
}

const fn default_profile_version() -> u32 {
    RETENTION_PROFILE_VERSION
}

const fn default_retention_floor() -> f64 {
    DEFAULT_RETENTION_FLOOR
}

/// Per-bank retention policy object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionProfile {
    /// Admission strictness level.
    #[serde(default)]
    pub verbosity: Verbosity,
    /// Bounded per-category threshold deltas on top of the verbosity delta.
    #[serde(default)]
    pub theta_offsets: BTreeMap<String, f64>,
    /// Admission-time duplicate cutoff.
    #[serde(default)]
    pub duplicate_tolerance_write: DuplicateTolerance,
    /// Read-time duplicate cap (stored here; honored by the recall-side dedup capability).
    #[serde(default)]
    pub duplicate_tolerance_read: DuplicateTolerance,
    /// Default recall scope (stored here; honored by the recall-side scope capability).
    #[serde(default)]
    pub recall_scope_default: RecallScope,
    /// Optional deterministic retention mission (keep/drop examples).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<RetentionMission>,
    /// Active flag for the retention curve applied by ranked retrieval shaping.
    ///
    /// Consumer: curve/ranking.
    #[serde(default = "default_true")]
    pub retention_curve: bool,
    /// Lowest multiplier the retention curve may apply, in `(0, 1]`.
    ///
    /// Consumer: curve/ranking.
    #[serde(default = "default_retention_floor")]
    pub retention_floor: f64,
    /// Default pack fidelity channel.
    ///
    /// Consumer: pack channel.
    #[serde(default)]
    pub pack_fidelity_default: PackFidelityDefault,
    /// Default provenance floor; `None` means no floor.
    ///
    /// Consumer: provenance floor default. A rung this build cannot read is
    /// resolved by `fail_closed_stored_floor` before a stored file is typed;
    /// see that function for why the leniency is not in the `Deserialize` impl.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance_floor_default: Option<ProvenanceFloorDefault>,
    /// On/off switch for the maintenance trigger.
    ///
    /// Consumer: maintenance trigger.
    #[serde(default = "default_true")]
    pub maintenance_trigger: bool,
    /// Schema version stamp.
    #[serde(default = "default_profile_version")]
    pub version: u32,
}

/// Resolve a stored provenance floor value, failing the floor closed.
///
/// Returns the strictest rung when `raw` names no rung and must be rewritten, or
/// `None` when `raw` already names a rung or means no floor and is left alone.
/// A missing key never reaches here; `null` means no floor.
///
/// The two surfaces that meet a rung name are not the same surface. The REQUEST
/// surface has a caller in front of it, so `ProvenanceFloorDefault::parse`
/// refuses a name outside the ladder as `invalid_argument` and the caller is
/// told. A STORED file has no caller left to tell: there, an unrecognized name
/// resolves to the strictest rung, `verified`, whose admitted set is contained
/// in every other rung's. A corrupt floor therefore narrows reads instead of
/// widening them, and because this one value is tolerated, the rest of the
/// file's profiles still load instead of being discarded with it.
///
/// The leniency is deliberately NOT a `deserialize_with` on the field. The
/// derived `Deserialize` is shared with the request surface:
/// `retention_profile_set` and a batch `profile_override` both reach it through
/// `serde_json::from_value::<RetentionProfile>`, so a field-level lenient reader
/// would coerce a mistyped rung on those calls too instead of refusing it.
/// Rewriting the raw document before it is typed keeps the strict reader on the
/// request path and confines the fallback to a stored file.
///
/// Serialization is untouched, so a file this code writes stays a file it reads.
/// A non-string value degrades the same way, so this field never fails a whole
/// store file.
pub(crate) fn fail_closed_stored_floor(raw: &Value) -> Option<ProvenanceFloorDefault> {
    match raw {
        Value::Null => None,
        Value::String(name) if ProvenanceFloorDefault::parse(name).is_ok() => None,
        _ => Some(ProvenanceFloorDefault::Verified),
    }
}

impl Default for RetentionProfile {
    fn default() -> Self {
        Self {
            verbosity: Verbosity::Balanced,
            theta_offsets: BTreeMap::new(),
            duplicate_tolerance_write: DuplicateTolerance::Balanced,
            duplicate_tolerance_read: DuplicateTolerance::Balanced,
            recall_scope_default: RecallScope::Full,
            mission: None,
            retention_curve: true,
            retention_floor: DEFAULT_RETENTION_FLOOR,
            pack_fidelity_default: PackFidelityDefault::Flow,
            provenance_floor_default: None,
            maintenance_trigger: true,
            version: RETENTION_PROFILE_VERSION,
        }
    }
}

impl RetentionProfile {
    /// Validate enum-independent bounds: version, known keys, offset range, floor
    /// range, and mission.
    ///
    /// Only the current version is accepted. A stored file with an older marker is
    /// converted before it is typed (see `retention_store`), so an older version
    /// reaching this check is a REQUEST that carried it and is refused.
    ///
    /// # Errors
    /// A version other than the current one, an invalid mission, an unknown offset
    /// key, an out-of-range offset, or an out-of-range retention floor.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.version != RETENTION_PROFILE_VERSION {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "retention profile version {} is not supported (expected {RETENTION_PROFILE_VERSION})",
                    self.version
                ),
            ));
        }
        if let Some(mission) = &self.mission {
            mission.validate()?;
        }
        if !self.retention_floor.is_finite()
            || self.retention_floor <= 0.0
            || self.retention_floor > 1.0
        {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "retention_floor must be finite and in (0, 1], got {}",
                    self.retention_floor
                ),
            ));
        }
        for (key, delta) in &self.theta_offsets {
            if !KNOWN_ADMISSION_KEYS.contains(&key.as_str()) {
                return Err(AmError::new(
                    ErrorCode::UnknownPath,
                    format!(
                        "theta_offsets key `{key}` must be a known category, episodic type, or belief key"
                    ),
                ));
            }
            if !delta.is_finite() || delta.abs() > MAX_THETA_OFFSET {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!(
                        "theta_offsets for `{key}` must be finite and within ±{MAX_THETA_OFFSET}"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Resolve this profile into a copyable admission snapshot.
    #[must_use]
    pub fn resolve(&self) -> ResolvedRetention {
        let mut offsets = [0.0_f64; ADMISSION_KEY_COUNT];
        for (key, delta) in &self.theta_offsets {
            if let Some(index) = key_index(key) {
                offsets[index] = *delta;
            }
        }
        let mission = ResolvedMission::from_mission(self.mission.as_ref());
        ResolvedRetention {
            theta_delta: self.verbosity.theta_delta(),
            novelty_floor: self
                .verbosity
                .novelty_floor()
                .max(self.duplicate_tolerance_write.write_novelty_floor()),
            offsets,
            profile_version: self.version,
            mission: if mission.is_empty() {
                None
            } else {
                Some(Arc::new(mission))
            },
            retention_curve: self.retention_curve,
            retention_floor: self.retention_floor,
            pack_fidelity_default: self.pack_fidelity_default,
            provenance_floor_default: self.provenance_floor_default,
            maintenance_trigger: self.maintenance_trigger,
        }
    }

    /// The stored read-side policy fields, wrapped so the active values and the
    /// capability that honors them are visible together.
    #[must_use]
    pub fn accepted_but_inert(&self) -> Value {
        json!({
            "duplicate_tolerance_read": self.duplicate_tolerance_read.as_str(),
            "recall_scope_default": self.recall_scope_default.as_str(),
            "honored_by": INERT_FIELDS_HONORED_BY,
            "status": "active",
            "note": "read-side scope and dedup are active; these values apply to retrieve/compose unless the call overrides them"
        })
    }
}

/// Index of an admission key in the resolved offset array.
#[must_use]
pub fn key_index(key: &str) -> Option<usize> {
    KNOWN_ADMISSION_KEYS.iter().position(|k| *k == key)
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod retention_tests;

#[cfg(test)]
#[path = "retention_fields_tests.rs"]
mod retention_fields_tests;
