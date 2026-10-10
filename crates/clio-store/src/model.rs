// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Persistence domain models for item envelopes, assoc edges, and vectors.

use serde::{Deserialize, Serialize};

use clio_types::{AmError, ErrorCode};

/// Fixed embedding width for Profile B/C (`bge-small-en-v1.5`).
pub const EMBEDDING_DIMS: usize = 384;

/// Marker for the fixed embedding dimensionality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingDims;

impl EmbeddingDims {
    /// Returns the fixed dimension count.
    #[must_use]
    pub const fn get(self) -> usize {
        EMBEDDING_DIMS
    }
}

/// Association edge kind (`assoc_kind`). Not an SPO triple kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssocKind {
    /// Co-activation reinforcement edge.
    Coactivation,
    /// Explicit `graph_link` edge.
    Explicit,
}

impl AssocKind {
    /// Stable wire/SQL string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coactivation => "coactivation",
            Self::Explicit => "explicit",
        }
    }

    /// Parse from SQL/wire value.
    ///
    /// # Errors
    /// Unknown kind strings.
    pub fn parse(raw: &str) -> Result<Self, clio_types::AmError> {
        match raw {
            "coactivation" => Ok(Self::Coactivation),
            "explicit" => Ok(Self::Explicit),
            other => Err(clio_types::AmError::new(
                clio_types::ErrorCode::InvalidArgument,
                format!("assoc_kind must be coactivation|explicit, got `{other}`"),
            )),
        }
    }
}

/// One opaque at-rest item row restored from a `ciphertext_backup` bundle.
///
/// The content body is the exporting node's ciphertext stored verbatim (no
/// re-encryption under the local KMS); all metadata columns are carried
/// so the restored row keeps its taxonomy, scores, and bi-temporal windows.
#[derive(Debug, Clone, PartialEq)]
pub struct OpaqueItemRecord {
    /// Stable item id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Data-subject id for key resolution.
    pub subject_id: String,
    /// `semantic` | `episodic`.
    pub kind: String,
    /// Semantic category, when semantic.
    pub category: Option<String>,
    /// Episodic type tag, when episodic.
    pub episodic_type: Option<String>,
    /// `fact` | `belief`.
    pub epistemic_kind: String,
    /// Provenance source type (beliefs).
    pub source_type: Option<String>,
    /// Confidence.
    pub confidence: Option<f64>,
    /// Five-factor admission score.
    pub admission_score: Option<f64>,
    /// Evidence reference.
    pub source_ref: Option<String>,
    /// Valid-time start.
    pub valid_from: String,
    /// Valid-time end.
    pub valid_until: Option<String>,
    /// Transaction-time start.
    pub tx_from: String,
    /// Transaction-time end.
    pub tx_until: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last-update timestamp.
    pub updated_at: String,
    /// Opaque at-rest content bytes from the exporting node (no DEK material).
    pub content_cipher: String,
}

/// Graph/structure metadata row for an `assoc_edge`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AssocEdgeRecord {
    /// Stable edge id.
    pub id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Source memory item id.
    pub source_id: String,
    /// Target memory item id.
    pub target_id: String,
    /// Association kind.
    pub assoc_kind: AssocKind,
    /// Optional relationship label (required identity part for explicit edges).
    pub relationship: Option<String>,
    /// Weight in `0.0..=1.0`.
    pub weight: f64,
    /// Last update timestamp (ISO-8601 UTC text).
    pub last_updated_at: String,
}

/// Bank-scoped identity columns for one item, read without content/DEK access.
///
/// Used by the operations-removal (`discard`) path for telemetry: the
/// caller learns what was removed without decrypting the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemIdentity {
    /// Item id.
    pub item_id: String,
    /// Semantic category, when the item is semantic (`None` for episodic).
    pub category: Option<String>,
    /// `fact` or `belief`.
    pub epistemic_kind: String,
}

/// Validity filter for the inspector listing.
///
/// "Invalidated" items are closed on the transaction axis (`tx_until` set) but
/// retained for history; they are never erased by supersession.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectValidity {
    /// Any validity state.
    #[default]
    All,
    /// Currently valid (`tx_until IS NULL`).
    Active,
    /// Closed on the transaction axis (`tx_until` set).
    Invalidated,
    /// Operationally discarded (`discarded_at` set logged removal).
    Discarded,
}

impl InspectValidity {
    /// Parse a caller-supplied validity filter.
    ///
    /// # Errors
    /// Unknown validity label.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "" | "all" => Ok(Self::All),
            "active" => Ok(Self::Active),
            "invalidated" => Ok(Self::Invalidated),
            "discarded" => Ok(Self::Discarded),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("validity must be all|active|invalidated|discarded, got `{other}`"),
            )),
        }
    }
}

/// Provenance-class filter for the inspector listing.
///
/// Facts are exactly the rows with `source_type` NULL (the schema CHECK and
/// item validation make belief-without-source unreachable); the other rungs
/// each select one stored `source_type` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectSource {
    /// Facts (`source_type IS NULL`).
    Fact,
    /// Beliefs the user stated (`user_stated`).
    UserStated,
    /// Beliefs the agent inferred (`agent_inferred`).
    AgentInferred,
    /// Beliefs a third party reported (`third_party`).
    ThirdParty,
}

impl InspectSource {
    /// Stored `source_type` value for a belief rung; `None` for [`InspectSource::Fact`].
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Fact => None,
            Self::UserStated => Some("user_stated"),
            Self::AgentInferred => Some("agent_inferred"),
            Self::ThirdParty => Some("third_party"),
        }
    }
}

/// Filter for [`Store::inspect_items`] (metadata-only listing).
#[derive(Debug, Clone)]
pub struct ItemInspectFilter {
    /// Required bank scope (tenant isolation).
    pub bank_id: String,
    /// Optional semantic category filter.
    pub category: Option<String>,
    /// Optional `fact` | `belief` filter.
    pub epistemic_kind: Option<String>,
    /// Optional provenance-class filter (`None` = any provenance).
    pub source_type: Option<InspectSource>,
    /// Validity-state filter.
    pub validity: InspectValidity,
    /// Optional lower bound on `created_at` (inclusive).
    pub time_start: Option<String>,
    /// Optional upper bound on `created_at` (exclusive: `[start, end)`).
    pub time_end: Option<String>,
    /// Include operationally discarded items (default excluded).
    pub include_discarded: bool,
    /// Deterministic page size.
    pub limit: usize,
    /// Deterministic page offset.
    pub offset: usize,
}

impl ItemInspectFilter {
    /// Bank-scoped filter with defaults (all validity, discarded excluded).
    #[must_use]
    pub fn new(bank_id: impl Into<String>) -> Self {
        Self {
            bank_id: bank_id.into(),
            category: None,
            epistemic_kind: None,
            source_type: None,
            validity: InspectValidity::All,
            time_start: None,
            time_end: None,
            include_discarded: false,
            limit: 100,
            offset: 0,
        }
    }
}

/// One metadata-only inspector row.
///
/// Never carries ciphertext, DEKs, or decrypted content: only the columns the
/// Listing needs (ids, taxonomy, scores, validity windows, refs).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemInspectRow {
    /// Item id.
    pub item_id: String,
    /// `semantic` | `episodic`.
    pub kind: String,
    /// Semantic category, when semantic.
    pub category: Option<String>,
    /// Episodic type tag, when episodic.
    pub episodic_type: Option<String>,
    /// `fact` | `belief`.
    pub epistemic_kind: String,
    /// Provenance source type (beliefs).
    pub source_type: Option<String>,
    /// Confidence (beliefs).
    pub confidence: Option<f64>,
    /// Five-factor admission score.
    pub admission_score: Option<f64>,
    /// Evidence reference.
    pub source_ref: Option<String>,
    /// Valid-time start.
    pub valid_from: String,
    /// Valid-time end (`None` = open).
    pub valid_until: Option<String>,
    /// Transaction-time start.
    pub tx_from: String,
    /// Transaction-time end (`None` = current).
    pub tx_until: Option<String>,
    /// Operationally discarded (not compliance-erased).
    pub discarded: bool,
    /// Hygiene-archived (excluded from default retrieve).
    pub archived: bool,
    /// Creation timestamp.
    pub created_at: String,
    /// Last-update timestamp.
    pub updated_at: String,
}

/// Dense vector slot tied to an item.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorSlot {
    /// Item id (PK).
    pub item_id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Embedding model id; must equal the active embedding space's model.
    pub model_id: String,
    /// Dense vector; length must equal the active embedding space width
    /// (default [`EMBEDDING_DIMS`]).
    pub embedding: Vec<f32>,
}

/// One KNN hit from a vector query hook.
#[derive(Debug, Clone, PartialEq)]
pub struct KnnHit {
    /// Matching item id.
    pub item_id: String,
    /// Backend distance (cosine distance for both drivers).
    pub distance: f64,
}

/// One lexical-search hit (normalized: higher `score` = better match).
///
/// SQLite FTS5 `bm25()` ranks lower-is-better and is negated on read;
/// Postgres `ts_rank_cd` already ranks higher-is-better. Slice-12 fusion
/// consumes ranks, so polarity only matters for primitive ordering.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Matching item id.
    pub item_id: String,
    /// Relevance where higher is better.
    pub score: f64,
}

/// One pending index-pipeline job (durable outbox row for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexJob {
    /// Bank isolation key.
    pub bank_id: String,
    /// Item awaiting dense + lexical indexing.
    pub item_id: String,
    /// Failed attempts so far (observability; backoff uses `last_attempt_at`).
    pub attempts: u32,
    /// Last worker error, when any.
    pub last_error: Option<String>,
}

/// Index pipeline coverage for one bank or the whole store.
///
/// Counts are PII-safe by construction: numbers and ids only, never text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexCoverage {
    /// Scope filter (`None` = whole store).
    pub bank_id: Option<String>,
    /// Open, non-discarded items carrying indexable dual content.
    pub indexable_items: u64,
    /// Dense vector rows.
    pub dense_rows: u64,
    /// Lexical rows (FTS5 rows / non-null `content_tsv`).
    pub lexical_rows: u64,
    /// Pending (never-attempted) queue depth.
    pub pending_jobs: u64,
    /// Queue rows that failed at least once and await retry.
    pub retry_jobs: u64,
    /// Most recent worker error text (ids/labels only).
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assoc_kind_parse_rejects_unknown() {
        assert_eq!(
            AssocKind::parse("coactivation").unwrap(),
            AssocKind::Coactivation
        );
        assert_eq!(AssocKind::parse("explicit").unwrap(), AssocKind::Explicit);
        assert!(AssocKind::parse("distilled").is_err());
        assert_eq!(EmbeddingDims.get(), 384);
    }

    #[test]
    fn inspect_source_rungs_map_one_to_one_to_source_type_values() {
        assert_eq!(InspectSource::Fact.as_str(), None);
        assert_eq!(InspectSource::UserStated.as_str(), Some("user_stated"));
        assert_eq!(
            InspectSource::AgentInferred.as_str(),
            Some("agent_inferred")
        );
        assert_eq!(InspectSource::ThirdParty.as_str(), Some("third_party"));
    }

    #[test]
    fn inspect_filter_defaults_and_validity_parse() {
        let filter = ItemInspectFilter::new("bank-a");
        assert_eq!(filter.bank_id, "bank-a");
        assert_eq!(filter.validity, InspectValidity::All);
        assert_eq!(filter.limit, 100);
        assert_eq!(filter.offset, 0);
        assert_eq!(InspectValidity::parse("").unwrap(), InspectValidity::All);
        assert_eq!(
            InspectValidity::parse("active").unwrap(),
            InspectValidity::Active
        );
        assert_eq!(
            InspectValidity::parse("invalidated").unwrap(),
            InspectValidity::Invalidated
        );
        assert!(InspectValidity::parse("maybe").is_err());
    }
}

#[cfg(test)]
#[path = "model_memory_item_tests.rs"]
mod model_memory_item_tests;
