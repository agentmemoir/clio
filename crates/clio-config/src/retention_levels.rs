// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for the retention level enums and their exact
//! strictness mapping onto admission thresholds and duplicate cutoffs.
//!
//! ## Owns
//! - `Verbosity`, `DuplicateTolerance`, `RecallScope` and their wire parsing
//! - `PackFidelityDefault` and `ProvenanceFloorDefault`, the pack channel and
//!   provenance-floor rung enums
//! - The verbosity/duplicate mapping constants those levels resolve to
//!
//! ## Does not own
//! - The profile object shape, validation, and defaults (see `retention`)
//! - The resolved snapshot consumed by admission (see `retention_resolved`)
//!
//! ## Boundary
//!
//! Changes to level names or mapping numbers should normally require changes here.
//! Changes to profile fields or resolution should not require changes here.
//!
//! Keep this module focused on the level enums and their mapping tables.

use crate::error::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

/// `verbosity=selective` threshold delta.
pub const SELECTIVE_THETA_DELTA: f64 = 0.08;

/// `verbosity=permissive` threshold delta.
pub const PERMISSIVE_THETA_DELTA: f64 = -0.08;

/// `verbosity=selective` novelty floor.
pub const SELECTIVE_NOVELTY_FLOOR: f64 = 0.15;

/// `duplicate_tolerance_write=strict` novelty floor.
pub const STRICT_WRITE_NOVELTY_FLOOR: f64 = 0.25;

/// Read-time token-set similarity cap for `duplicate_tolerance_read=strict`.
pub const STRICT_READ_SIMILARITY: f64 = 0.80;

/// Read-time token-set similarity cap for `duplicate_tolerance_read=balanced`.
pub const BALANCED_READ_SIMILARITY: f64 = 0.90;

/// Read-time token-set similarity cap for `duplicate_tolerance_read=lenient`.
pub const LENIENT_READ_SIMILARITY: f64 = 0.95;

/// Three-level admission strictness knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verbosity {
    /// Strictest: higher thresholds and a novelty floor.
    Selective,
    /// Default: zero delta, current behavior.
    #[default]
    Balanced,
    /// Loosest: lower thresholds; intended for ingest-debug banks only.
    Permissive,
}

impl Verbosity {
    /// Parse the wire value.
    ///
    /// # Errors
    /// Unknown level names.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "selective" => Ok(Self::Selective),
            "balanced" => Ok(Self::Balanced),
            "permissive" => Ok(Self::Permissive),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("verbosity must be selective|balanced|permissive, got `{other}`"),
            )),
        }
    }

    /// Wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Selective => "selective",
            Self::Balanced => "balanced",
            Self::Permissive => "permissive",
        }
    }

    /// Additive `theta` delta for this level.
    #[must_use]
    pub const fn theta_delta(self) -> f64 {
        match self {
            Self::Selective => SELECTIVE_THETA_DELTA,
            Self::Balanced => 0.0,
            Self::Permissive => PERMISSIVE_THETA_DELTA,
        }
    }

    /// Novelty floor contributed by this level.
    #[must_use]
    pub const fn novelty_floor(self) -> f64 {
        match self {
            Self::Selective => SELECTIVE_NOVELTY_FLOOR,
            Self::Balanced | Self::Permissive => 0.0,
        }
    }
}

/// Three-level duplicate tolerance for admission-time novelty cutoffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateTolerance {
    /// Reject near-duplicates aggressively.
    Strict,
    /// Default: current behavior.
    #[default]
    Balanced,
    /// Explicitly tolerate near-duplicates.
    Lenient,
}

impl DuplicateTolerance {
    /// Parse the wire value.
    ///
    /// # Errors
    /// Unknown level names.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "strict" => Ok(Self::Strict),
            "balanced" => Ok(Self::Balanced),
            "lenient" => Ok(Self::Lenient),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("duplicate_tolerance must be strict|balanced|lenient, got `{other}`"),
            )),
        }
    }

    /// Wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Balanced => "balanced",
            Self::Lenient => "lenient",
        }
    }

    /// Write-side novelty floor for this level.
    #[must_use]
    pub const fn write_novelty_floor(self) -> f64 {
        match self {
            Self::Strict => STRICT_WRITE_NOVELTY_FLOOR,
            Self::Balanced | Self::Lenient => 0.0,
        }
    }

    /// Read-time token-set similarity cap for this level.
    ///
    /// Applied only to the returned page and only when a scope preset or
    /// `prefer_consolidated` is active, or when the level is `strict` or
    /// `lenient`; the neutral `balanced` default applies no cap on a plain
    /// full call, so pre-existing recall output is unchanged.
    #[must_use]
    pub const fn read_similarity_cap(self) -> f64 {
        match self {
            Self::Strict => STRICT_READ_SIMILARITY,
            Self::Balanced => BALANCED_READ_SIMILARITY,
            Self::Lenient => LENIENT_READ_SIMILARITY,
        }
    }
}

/// Recall scope default stored on the profile; honored by the recall-side scope capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallScope {
    /// Rank and return normally.
    #[default]
    Full,
    /// Return consolidated units only.
    ConsolidatedOnly,
}

impl RecallScope {
    /// Parse the wire value.
    ///
    /// # Errors
    /// Unknown scope names.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "full" => Ok(Self::Full),
            "consolidated_only" => Ok(Self::ConsolidatedOnly),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("recall_scope must be full|consolidated_only, got `{other}`"),
            )),
        }
    }

    /// Wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::ConsolidatedOnly => "consolidated_only",
        }
    }
}

/// Default pack fidelity channel stored on the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackFidelityDefault {
    /// Default channel.
    #[default]
    Flow,
    /// Exact channel.
    Exact,
}

impl PackFidelityDefault {
    /// Parse the wire value.
    ///
    /// # Errors
    /// Unknown channel names.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "flow" => Ok(Self::Flow),
            "exact" => Ok(Self::Exact),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("pack_fidelity_default must be flow|exact, got `{other}`"),
            )),
        }
    }

    /// Wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::Exact => "exact",
        }
    }
}

/// Closed ladder of provenance rungs usable as a profile floor.
///
/// Four rungs, no more: on the request path an unknown rung is refused rather
/// than aliased. The strength order is the declaration order, so the derived
/// `Ord` reads `Verified < Stated < ThirdParty < Inferred`: a lower value is
/// the stricter floor, and each rung's admitted set is contained in the next
/// one's (`verified` inside `stated` inside `third_party` inside `inferred`). A
/// fifth rung is not silent: the exhaustive `match` in the level tests stops
/// the build, and the pinned ordinals make any insertion have to be declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceFloorDefault {
    /// Strongest rung.
    Verified,
    /// User-stated rung.
    Stated,
    /// Third-party rung.
    ThirdParty,
    /// Weakest rung.
    Inferred,
}

impl ProvenanceFloorDefault {
    /// Parse the wire value.
    ///
    /// # Errors
    /// Unknown rung names.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "verified" => Ok(Self::Verified),
            "stated" => Ok(Self::Stated),
            "third_party" => Ok(Self::ThirdParty),
            "inferred" => Ok(Self::Inferred),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "provenance_floor_default must be verified|stated|third_party|inferred, got `{other}`"
                ),
            )),
        }
    }

    /// Wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Stated => "stated",
            Self::ThirdParty => "third_party",
            Self::Inferred => "inferred",
        }
    }
}

#[cfg(test)]
#[path = "retention_levels_tests.rs"]
mod retention_levels_tests;
