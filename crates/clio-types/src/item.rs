// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for memory-item domain types and structural validation.
//!
//! ## Owns
//! - Semantic/episodic enums and `MemoryItem` shape
//! - `epistemic_kind` / `source_type` well-formedness
//!
//! ## Does not own
//! - Persistence, encryption, or admission scoring
//!
//! ## Boundary
//!
//! Changes to item field shapes and structural rules should normally require changes here.
//! Changes to storage drivers should not require changes here.

use crate::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level store kind (semantic vs episodic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// Semantic categories.
    Semantic,
    /// Episodic units with an episodic type tag (not a sixth semantic category).
    Episodic,
}

impl ItemKind {
    /// Stable wire/SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Episodic => "episodic",
        }
    }

    /// Parse from SQL/wire.
    ///
    /// # Errors
    /// Unknown kind.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "semantic" => Ok(Self::Semantic),
            "episodic" => Ok(Self::Episodic),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("kind must be semantic|episodic, got `{other}`"),
            )),
        }
    }
}

/// Defines [`SemanticCategory`] and its derived tables from one list.
///
/// The invocation at the bottom of this block is the single source of truth:
/// adding or removing a category there updates the enum variants, the `ALL`
/// help table, the `WIRE` strings, `as_str`, and `parse` together. The CLI's
/// `--category` help and the SQL `CHECK` parity test read these tables, so they
/// cannot drift from the whitelist.
macro_rules! semantic_categories {
    ($( $variant:ident => $wire:literal, $description:literal; )+) => {
        /// Semantic category whitelist. Gate enforcement is elsewhere.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum SemanticCategory {
            $( $variant, )+
        }

        impl SemanticCategory {
            /// Every category with its wire string and one-line description, in
            /// canonical order. Single source of truth for `--category` help.
            pub const ALL: &[(Self, &'static str, &'static str)] =
                &[ $( (Self::$variant, $wire, $description), )+ ];

            /// Wire/SQL strings in canonical order (parallel to `ALL`).
            pub const WIRE: &[&'static str] = &[ $( $wire, )+ ];

            /// Stable wire/SQL string.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire, )+
                }
            }

            /// Parse from SQL/wire.
            ///
            /// # Errors
            /// Unknown category.
            pub fn parse(raw: &str) -> Result<Self, AmError> {
                match raw {
                    $( $wire => Ok(Self::$variant), )+
                    other => Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        format!("category must be {}, got `{other}`", Self::WIRE.join("|")),
                    )),
                }
            }
        }
    };
}

semantic_categories! {
    TaskSpec => "task_spec", "Task specifications";
    Schema => "schema", "Data schemas";
    ToolConfig => "tool_config", "Tool configurations";
    OutputConstraint => "output_constraint", "Output constraints";
    Persona => "persona", "User model";
}

/// Episodic type tag (not a semantic category; belief is not a tag).
///
/// Note: the tag value `gist` names an episodic *kind of record*. The prose
/// dual-representation half is a separate field also called gist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodicType {
    Triple,
    Gist,
    Task,
    Failure,
    Temporal,
}

impl EpisodicType {
    /// Stable wire/SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Triple => "triple",
            Self::Gist => "gist",
            Self::Task => "task",
            Self::Failure => "failure",
            Self::Temporal => "temporal",
        }
    }

    /// Parse from SQL/wire.
    ///
    /// # Errors
    /// Unknown tag.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "triple" => Ok(Self::Triple),
            "gist" => Ok(Self::Gist),
            "task" => Ok(Self::Task),
            "failure" => Ok(Self::Failure),
            "temporal" => Ok(Self::Temporal),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("episodic_type must be triple|gist|task|failure|temporal, got `{other}`"),
            )),
        }
    }
}

/// Fact vs belief. Never used as `update_rule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicKind {
    Fact,
    Belief,
}

impl EpistemicKind {
    /// Stable wire/SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Belief => "belief",
        }
    }

    /// Parse from SQL/wire.
    ///
    /// # Errors
    /// Unknown kind.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "fact" => Ok(Self::Fact),
            "belief" => Ok(Self::Belief),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("epistemic_kind must be fact|belief, got `{other}`"),
            )),
        }
    }
}

/// Defines [`SourceType`] and its derived tables from one list.
///
/// The invocation at the bottom of this block is the single source of truth:
/// adding or removing a value there updates the enum variants, the `ALL` table,
/// the `WIRE` strings, `as_str`, and `parse` together. The CLI's `--source-type`
/// values and every message that names them read these tables, so the value list
/// cannot drift — the same shape `semantic_categories!` uses above.
macro_rules! source_types {
    ($( $variant:ident => $wire:literal; )+) => {
        /// Belief provenance (required iff `epistemic_kind = belief`).
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum SourceType {
            $( $variant, )+
        }

        impl SourceType {
            /// Every value in canonical order.
            pub const ALL: &[Self] = &[ $( Self::$variant, )+ ];

            /// Wire/SQL strings in canonical order (parallel to `ALL`).
            pub const WIRE: &[&'static str] = &[ $( $wire, )+ ];

            /// Stable wire/SQL string.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $wire, )+
                }
            }

            /// Parse from SQL/wire.
            ///
            /// # Errors
            /// Unknown source type.
            pub fn parse(raw: &str) -> Result<Self, AmError> {
                match raw {
                    $( $wire => Ok(Self::$variant), )+
                    other => Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        format!(
                            "source_type must be {}, got `{other}`",
                            Self::WIRE.join("|")
                        ),
                    )),
                }
            }
        }
    };
}

source_types! {
    UserStated => "user_stated";
    AgentInferred => "agent_inferred";
    ThirdParty => "third_party";
}

/// Published maximum size for one source `context` value, in UTF-8 bytes
/// Deployments may lower but not raise it.
pub const CONTEXT_MAX_BYTES: usize = 4096;

/// Validate one source `context` value: present values must be non-empty and at
/// most [`CONTEXT_MAX_BYTES`] UTF-8 bytes (never truncated).
///
/// # Errors
/// `InvalidArgument` for an empty value; `OutOfRange` when the UTF-8 byte
/// length exceeds the published maximum. Rust `String`s are always valid UTF-8,
/// so an invalid-UTF-8 input cannot reach this function.
pub fn validate_context(context: &str) -> Result<(), AmError> {
    if context.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "context must not be empty when supplied (omit it instead)",
        ));
    }
    if context.len() > CONTEXT_MAX_BYTES {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!(
                "context must be at most {CONTEXT_MAX_BYTES} UTF-8 bytes, got {}",
                context.len()
            ),
        ));
    }
    Ok(())
}

/// Domain memory item with separate snapshot and gist halves.
///
/// Repository writes that accept this type are **ungated primitives**: public
/// long-term tools must wrap them with admission gates before commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryItem {
    /// Stable item id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Data-subject id for DEK selection.
    pub subject_id: String,
    /// Semantic vs episodic.
    pub kind: ItemKind,
    /// Required when `kind = semantic`.
    pub category: Option<SemanticCategory>,
    /// Required when `kind = episodic`.
    pub episodic_type: Option<EpisodicType>,
    /// Fact or belief.
    pub epistemic_kind: EpistemicKind,
    /// Mandatory when belief; must be absent when fact (reject, do not strip).
    pub source_type: Option<SourceType>,
    /// Optional confidence in `0.0..=1.0`.
    pub confidence: Option<f64>,
    /// Admission score placeholder (filled by later gates).
    pub admission_score: Option<f64>,
    /// Provenance / source turn or tool call.
    pub source_ref: Option<String>,
    /// Optional bounded descriptive source context. Descriptive only:
    /// never a category, truth claim, span haystack, or retrieval budget.
    ///
    /// `default` keeps rows/bundles written before the field existed readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Authoritative structured half (exact values).
    pub snapshot: Option<Value>,
    /// Non-authoritative prose half.
    pub gist: Option<String>,
    /// Valid-time start (ISO text).
    pub valid_from: String,
    /// Valid-time end (open = None).
    pub valid_until: Option<String>,
    /// Transaction-time start.
    pub tx_from: String,
    /// Transaction-time end (open = None).
    pub tx_until: Option<String>,
    /// Created timestamp.
    pub created_at: String,
    /// Updated timestamp.
    pub updated_at: String,
}

impl MemoryItem {
    /// Structural well-formedness (not admission scoring).
    ///
    /// # Errors
    /// Invalid kind/category pairing, `source_type` rules, empty ids, bad ranges.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.id.is_empty() || self.bank_id.is_empty() || self.subject_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "item id, bank_id, and subject_id are required",
            ));
        }
        if self.valid_from.is_empty() || self.tx_from.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "valid_from and tx_from are required",
            ));
        }
        match self.kind {
            ItemKind::Semantic => {
                if self.category.is_none() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "semantic items require category",
                    ));
                }
                if self.episodic_type.is_some() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "semantic items must not set episodic_type",
                    ));
                }
            }
            ItemKind::Episodic => {
                if self.episodic_type.is_none() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "episodic items require episodic_type",
                    ));
                }
                if self.category.is_some() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "episodic items must not set category",
                    ));
                }
            }
        }
        match self.epistemic_kind {
            EpistemicKind::Belief => {
                if self.source_type.is_none() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        format!(
                            "belief items require source_type ({})",
                            SourceType::WIRE.join("|")
                        ),
                    ));
                }
            }
            EpistemicKind::Fact => {
                if self.source_type.is_some() {
                    return Err(AmError::new(
                        ErrorCode::InvalidArgument,
                        "fact items must not include source_type; set it only for belief",
                    ));
                }
            }
        }
        for (label, score) in [
            ("confidence", self.confidence),
            ("admission_score", self.admission_score),
        ] {
            check_unit_interval(label, score)?;
        }
        if let Some(context) = &self.context {
            validate_context(context)?;
        }
        if let Some(gist) = &self.gist {
            crate::validate_gist(gist)?;
        }
        Ok(())
    }
}

fn check_unit_interval(label: &str, score: Option<f64>) -> Result<(), AmError> {
    let Some(v) = score else {
        return Ok(());
    };
    if (0.0..=1.0).contains(&v) {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::OutOfRange,
        format!("{label} must be in 0.0..=1.0"),
    ))
}

#[cfg(test)]
#[path = "item_tests.rs"]
mod item_tests;
