// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the frozen retrieval response field contract
//! consumers see for facts and beliefs.
//!
//! ## Owns
//! - `RetrieveHit` shape and the item / belief-object mapping
//! - The optional descriptive `context` hit metadata, mapped from the
//!   stored item only; beliefs carry none
//! - The optional bounded `exact_render` hit rendering, an opt-in field this
//!   crate does not produce
//! - The `requires_confidence_check` consumer hint for beliefs
//!
//! ## Does not own
//! - Hybrid retrieval ranking or compose
//! - Producing `exact_render` text or its byte cap (`clio-retrieve` owns both)
//! - Belief persistence or admission
//!
//! ## Boundary
//!
//! Changes to response fields on reads should normally require changes here.
//! Changes to retrieval orchestration should not require changes here.
//!
//! Keep this module focused on the frozen fact/belief read contract.

use crate::belief::BeliefObject;
use crate::error::{AmError, ErrorCode};
use crate::item::{EpistemicKind, MemoryItem, SourceType};
use serde::{Deserialize, Serialize};

/// Retrieval domain filter for the dense/lexical search primitives.
///
/// Maps the `retrieve` domain vocabulary onto the item `kind` /
/// `category` / `episodic_type` columns so backend search SQL can filter
/// in-query. This is a search filter — it never gates admission or storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchDomain {
    /// Semantic items (any category).
    Semantic,
    /// All episodic items.
    Episodic,
    /// Episodic `task` items.
    Task,
    /// Episodic `failure` items (`FailureRecord` carriers).
    Failure,
    /// Episodic `temporal` items.
    Temporal,
    /// Semantic `persona` category items.
    Persona,
}

impl SearchDomain {
    /// Stable wire/SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Episodic => "episodic",
            Self::Task => "task",
            Self::Failure => "failure",
            Self::Temporal => "temporal",
            Self::Persona => "persona",
        }
    }

    /// Parse from wire value.
    ///
    /// # Errors
    /// Unknown domain strings.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "semantic" => Ok(Self::Semantic),
            "episodic" => Ok(Self::Episodic),
            "task" => Ok(Self::Task),
            "failure" => Ok(Self::Failure),
            "temporal" => Ok(Self::Temporal),
            "persona" => Ok(Self::Persona),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "domain must be semantic|episodic|task|failure|temporal|persona, got `{other}`"
                ),
            )),
        }
    }

    /// `(kind, category, episodic_type)` SQL predicate parts for this domain.
    ///
    /// `None` parts mean "no filter" on that column.
    #[must_use]
    pub const fn item_predicates(
        self,
    ) -> (&'static str, Option<&'static str>, Option<&'static str>) {
        match self {
            Self::Semantic => ("semantic", None, None),
            Self::Episodic => ("episodic", None, None),
            Self::Task => ("episodic", None, Some("task")),
            Self::Failure => ("episodic", None, Some("failure")),
            Self::Temporal => ("episodic", None, Some("temporal")),
            Self::Persona => ("semantic", Some("persona"), None),
        }
    }

    /// Parse a list of domain strings, failing closed on the first unknown.
    ///
    /// # Errors
    /// Unknown domain strings.
    pub fn parse_all(raw: &[String]) -> Result<Vec<Self>, AmError> {
        raw.iter().map(|d| Self::parse(d)).collect()
    }
}

/// Read-side hit carrying epistemic kind so consumers never confuse fact and belief.
///
/// Facts serialize `epistemic_kind` and omit `source_type` / `confidence`
/// (facts keep no confidence trajectory). Beliefs add the current
/// `source_type`, current `confidence`, and a non-normative
/// `requires_confidence_check` hint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrieveHit {
    /// Item or belief id.
    pub item_id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// `semantic` | `episodic` for items, `belief` for belief objects.
    pub kind: String,
    /// Semantic category when `kind = semantic`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Episodic type tag when `kind = episodic`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episodic_type: Option<String>,
    /// `fact` or `belief`.
    pub epistemic_kind: EpistemicKind,
    /// Current provenance; present only when `epistemic_kind = belief`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_type: Option<SourceType>,
    /// Current confidence; present only when `epistemic_kind = belief`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// Non-authoritative prose half; the proposition for a belief object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gist: Option<String>,
    /// Optional bounded descriptive source context surfaced as hit
    /// metadata. Descriptive only: never part of the truth/epistemic semantics.
    ///
    /// Carried on the read hit, but deliberately not rendered into any
    /// model-facing compose pack (packs stay gist-only), so the budget
    /// contract is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Optional bounded rendering of this item's stored snapshot, produced only
    /// when the request asked for it.
    ///
    /// This is content. It is bounded: the cap is measured in **bytes** of the
    /// serialized rendering, and a rendering that does not fit within the cap is
    /// omitted whole rather than shortened, so this field is never a truncated
    /// snapshot.
    ///
    /// It is a rendering, not the lossless half of a split: it carries the
    /// snapshot in one string for display or citation, and it is never an
    /// authoritative substitute for `get_snapshot`. A caller that needs the
    /// exact stored value reads `get_snapshot`; this field never replaces that
    /// read and never asserts that what it shows is byte-for-byte what is
    /// stored.
    ///
    /// The token cost is an upper bound: a rendering of at most the cap's bytes
    /// holds at most that many characters, which the `estimate_tokens` heuristic
    /// charges at `cap / 4` tokens whichever text it holds. That heuristic
    /// counts characters and the cap counts bytes, so any rendering of 509 to
    /// 512 characters reaches that figure even while it holds multi-byte
    /// characters; only a rendering of fewer than 509 characters lands below
    /// it.
    ///
    /// Absent — not null, not empty — unless the request asked and the
    /// rendering fit, so a request that does not ask leaves every hit exactly
    /// as it was before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_render: Option<String>,
    /// True when the caller MUST threshold-check before treating content as fact.
    pub requires_confidence_check: bool,
}

impl RetrieveHit {
    /// Map a stored memory item to the response contract.
    #[must_use]
    pub fn from_item(item: &MemoryItem) -> Self {
        let is_belief = item.epistemic_kind == EpistemicKind::Belief;
        Self {
            item_id: item.id.clone(),
            bank_id: item.bank_id.clone(),
            kind: item.kind.as_str().to_owned(),
            category: item.category.map(|c| c.as_str().to_owned()),
            episodic_type: item.episodic_type.map(|t| t.as_str().to_owned()),
            epistemic_kind: item.epistemic_kind,
            source_type: if is_belief { item.source_type } else { None },
            confidence: if is_belief { item.confidence } else { None },
            gist: item.gist.clone(),
            context: item.context.clone(),
            // The rendering is opt-in and cap-bounded; this mapper never reads
            // the snapshot, so the producing crate attaches it afterwards.
            exact_render: None,
            requires_confidence_check: is_belief,
        }
    }

    /// Map a belief object to the response contract.
    #[must_use]
    pub fn from_belief(belief: &BeliefObject) -> Self {
        Self {
            item_id: belief.id.clone(),
            bank_id: belief.bank_id.clone(),
            kind: "belief".to_owned(),
            category: None,
            episodic_type: None,
            epistemic_kind: EpistemicKind::Belief,
            source_type: belief.current_source_type(),
            confidence: belief.current_confidence(),
            gist: Some(belief.proposition.clone()),
            context: belief.context.clone(),
            exact_render: None,
            requires_confidence_check: true,
        }
    }
}

#[cfg(test)]
#[path = "read_tests.rs"]
mod read_tests;
