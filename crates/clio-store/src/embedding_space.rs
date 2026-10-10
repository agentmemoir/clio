// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the active embedding space record: the
//! `(model_id, dims)` pair the derived vector table is built for, plus the
//! width-driven index-mode classification.
//!
//! ## Owns
//! - `EmbeddingSpace` resolution from configured/recorded values with fail-closed validation
//! - `schema_settings` key names for the recorded space
//! - `IndexMode` selection and the storage-width ceilings
//!
//! ## Does not own
//! - Vector-table DDL generation (`vector_ddl`)
//! - Reading/writing `schema_settings` rows (backend modules)
//! - Embedding transport or provider selection (the `clio-index` crate)
//!
//! ## Boundary
//!
//! Changes to space identity, width limits, or index-mode thresholds should
//! normally require changes here. Changes to backend SQL or DDL text should not.
//!
//! Keep this module focused on embedding-space identity and width policy.

use crate::model::EmbeddingDims;
use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

/// Default embedding model id (`bge-small-en-v1.5`, Profile B/C).
pub const DEFAULT_EMBEDDING_MODEL: &str = "BAAI/bge-small-en-v1.5";
/// `schema_settings` key holding the recorded embedding model id.
pub const SETTING_EMBEDDING_MODEL: &str = "embedding_model";
/// `schema_settings` key holding the recorded embedding width.
pub const SETTING_EMBEDDING_DIM: &str = "embedding_dim";
/// Highest width the vector column/storage engine accepts (pgvector storage ceiling).
pub const MAX_VECTOR_DIMS: usize = 16_000;
/// Highest width that can carry a plain `vector` HNSW index.
pub const ANN_VECTOR_MAX_DIMS: usize = 2_000;
/// Highest width that can carry a `halfvec` expression index.
pub const ANN_HALFVEC_MAX_DIMS: usize = 4_000;

/// How dense search is served for a width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexMode {
    /// Plain `vector` HNSW index (`dims <= 2000`).
    AnnVector,
    /// `halfvec` expression index (`2000 < dims <= 4000`).
    AnnHalfvec,
    /// No ANN index; exact scan (`dims > 4000`).
    ExactScan,
}

impl IndexMode {
    /// Classify a width into its index mode.
    #[must_use]
    pub const fn for_dims(dims: usize) -> Self {
        if dims <= ANN_VECTOR_MAX_DIMS {
            Self::AnnVector
        } else if dims <= ANN_HALFVEC_MAX_DIMS {
            Self::AnnHalfvec
        } else {
            Self::ExactScan
        }
    }

    /// Stable wire/SQL label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnnVector => "ann_vector",
            Self::AnnHalfvec => "ann_halfvec",
            Self::ExactScan => "exact_scan",
        }
    }

    /// True when an ANN index serves this width (either index flavour).
    #[must_use]
    pub const fn has_ann_index(self) -> bool {
        !matches!(self, Self::ExactScan)
    }
}

/// The active embedding space: exactly one `(model_id, dims)` per deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingSpace {
    /// Embedding model id recorded on vector rows.
    pub model_id: String,
    /// Dense vector width.
    pub dims: usize,
}

impl Default for EmbeddingSpace {
    fn default() -> Self {
        Self::default_space()
    }
}

impl EmbeddingSpace {
    /// The built-in default space (`bge-small-en-v1.5`, 384).
    #[must_use]
    pub fn default_space() -> Self {
        Self {
            model_id: DEFAULT_EMBEDDING_MODEL.to_owned(),
            dims: EmbeddingDims.get(),
        }
    }

    /// Validate a configured space; empty model or out-of-range width fails closed.
    ///
    /// # Errors
    /// Empty model id, zero width, or width above [`MAX_VECTOR_DIMS`].
    pub fn new(model_id: impl Into<String>, dims: usize) -> Result<Self, AmError> {
        let model_id = model_id.into();
        if model_id.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.model must be a non-empty model id",
            ));
        }
        if dims == 0 {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.dims must be a positive dimension",
            ));
        }
        if dims > MAX_VECTOR_DIMS {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "embed.dims must be at most {MAX_VECTOR_DIMS} (storage ceiling); got {dims}"
                ),
            ));
        }
        Ok(Self { model_id, dims })
    }

    /// Resolve a space from raw configured strings (both required together).
    ///
    /// `None` when neither value is present; a partial or unparsable value is an
    /// error so a half-configured space never silently falls back to the default.
    ///
    /// # Errors
    /// Model without width, width without model, or a non-numeric width.
    pub fn from_configured(
        model: Option<&str>,
        dims: Option<&str>,
    ) -> Result<Option<Self>, AmError> {
        match (model.map(str::trim), dims.map(str::trim)) {
            (None, None) => Ok(None),
            (Some(m), Some(d)) if !m.is_empty() && !d.is_empty() => {
                let parsed = d.parse::<usize>().map_err(|_| {
                    AmError::new(
                        ErrorCode::ConfigCorrupt,
                        format!("embed.dims must be a positive integer, got `{d}`"),
                    )
                })?;
                Ok(Some(Self::new(m, parsed)?))
            }
            (Some(""), _) => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.model must be a non-empty model id",
            )),
            _ => Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "embed.model and embed.dims must both be set to select an embedding space",
            )),
        }
    }

    /// Index mode serving this width.
    #[must_use]
    pub const fn index_mode(&self) -> IndexMode {
        IndexMode::for_dims(self.dims)
    }

    /// The width.
    #[must_use]
    pub const fn dims(&self) -> usize {
        self.dims
    }

    /// The model id.
    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Fail closed when `dims` is not this space's width.
    ///
    /// # Errors
    /// Width mismatch (never truncated or padded).
    pub fn require_dims(&self, dims: usize) -> Result<(), AmError> {
        if dims != self.dims {
            return Err(AmError::new(
                ErrorCode::OutOfRange,
                format!(
                    "embedding width {dims} does not match the active space (model {} dims {}); \
                     a width mismatch is an error, never truncated or padded",
                    self.model_id, self.dims
                ),
            ));
        }
        Ok(())
    }

    /// Fail closed when `model_id` is not this space's model.
    ///
    /// Only one space is active per deployment, so a row stamped with a
    /// different model is never stored and never silently mixed in.
    ///
    /// # Errors
    /// Model mismatch.
    pub fn require_model(&self, model_id: &str) -> Result<(), AmError> {
        if model_id != self.model_id {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "embedding model `{model_id}` does not match the active space model \
                     `{}` @ {} dims; a foreign model id is an error, never silently mixed in",
                    self.model_id, self.dims
                ),
            ));
        }
        Ok(())
    }
}

/// Outcome of reconciling the configured space with the recorded one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSwitchReport {
    /// Space that was active before the reconcile.
    pub from: EmbeddingSpace,
    /// Space requested by configuration.
    pub to: EmbeddingSpace,
    /// True when the vector table was dropped and recreated.
    pub switched: bool,
    /// Derived vector rows discarded by the switch (0 when it was a no-op).
    pub purged_rows: u64,
}

/// Refusal error shared by both backends' unconfirmed space-switch paths.
pub(crate) fn refused_switch(from: &EmbeddingSpace, to: &EmbeddingSpace) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "configured embedding space ({} @ {} dims) differs from the active space \
             ({} @ {} dims); switching purges the derived dense index and requires the \
             explicit opt-in embed.switch_space=true, then a dense reindex",
            to.model_id, to.dims, from.model_id, from.dims
        ),
    )
}

/// Emit one PII-free operational event for a completed switch (ids/counts only).
pub(crate) fn log_switch(from: &EmbeddingSpace, to: &EmbeddingSpace, purged: u64) {
    let from_dims = from.dims.to_string();
    let to_dims = to.dims.to_string();
    let purged = purged.to_string();
    clio_config::log::info(
        "embedding_space_switch",
        &[
            ("from_model", from.model_id.as_str()),
            ("from_dims", from_dims.as_str()),
            ("to_model", to.model_id.as_str()),
            ("to_dims", to_dims.as_str()),
            ("purged_rows", purged.as_str()),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_space_is_the_documented_default() {
        let space = EmbeddingSpace::default_space();
        assert_eq!(space.model_id(), DEFAULT_EMBEDDING_MODEL);
        assert_eq!(space.dims(), EmbeddingDims.get());
        assert_eq!(EmbeddingSpace::default(), space);
        assert_eq!(space.index_mode(), IndexMode::AnnVector);
    }

    #[test]
    fn new_rejects_empty_model_zero_and_over_ceiling() {
        assert_eq!(
            EmbeddingSpace::new("  ", 384).unwrap_err().code,
            ErrorCode::ConfigCorrupt
        );
        assert_eq!(
            EmbeddingSpace::new("m", 0).unwrap_err().code,
            ErrorCode::ConfigCorrupt
        );
        assert_eq!(
            EmbeddingSpace::new("m", MAX_VECTOR_DIMS + 1)
                .unwrap_err()
                .code,
            ErrorCode::OutOfRange
        );
        assert_eq!(
            EmbeddingSpace::new("m", MAX_VECTOR_DIMS).unwrap().dims(),
            16_000
        );
    }

    #[test]
    fn index_mode_thresholds() {
        assert_eq!(IndexMode::for_dims(384), IndexMode::AnnVector);
        assert_eq!(IndexMode::for_dims(2_000), IndexMode::AnnVector);
        assert_eq!(IndexMode::for_dims(2_001), IndexMode::AnnHalfvec);
        assert_eq!(IndexMode::for_dims(4_000), IndexMode::AnnHalfvec);
        assert_eq!(IndexMode::for_dims(4_001), IndexMode::ExactScan);
        assert_eq!(IndexMode::for_dims(16_000), IndexMode::ExactScan);
        assert_eq!(IndexMode::AnnVector.as_str(), "ann_vector");
        assert_eq!(IndexMode::AnnHalfvec.as_str(), "ann_halfvec");
        assert_eq!(IndexMode::ExactScan.as_str(), "exact_scan");
        assert!(IndexMode::AnnVector.has_ann_index());
        assert!(IndexMode::AnnHalfvec.has_ann_index());
        assert!(!IndexMode::ExactScan.has_ann_index());
    }

    #[test]
    fn from_configured_shapes() {
        assert_eq!(EmbeddingSpace::from_configured(None, None).unwrap(), None);
        let space = EmbeddingSpace::from_configured(Some("m"), Some("1536"))
            .unwrap()
            .expect("configured space");
        assert_eq!(space.dims(), 1536);
        assert_eq!(space.model_id(), "m");
        assert_eq!(
            EmbeddingSpace::from_configured(Some(""), Some("384"))
                .unwrap_err()
                .code,
            ErrorCode::ConfigCorrupt
        );
        assert_eq!(
            EmbeddingSpace::from_configured(Some("m"), None)
                .unwrap_err()
                .code,
            ErrorCode::ConfigCorrupt
        );
        assert_eq!(
            EmbeddingSpace::from_configured(Some("m"), Some("nope"))
                .unwrap_err()
                .code,
            ErrorCode::ConfigCorrupt
        );
        assert_eq!(
            EmbeddingSpace::from_configured(Some("m"), Some("0"))
                .unwrap_err()
                .code,
            ErrorCode::ConfigCorrupt
        );
        assert_eq!(
            EmbeddingSpace::from_configured(Some("m"), Some("16001"))
                .unwrap_err()
                .code,
            ErrorCode::OutOfRange
        );
    }

    #[test]
    fn require_dims_fails_closed_on_mismatch() {
        let space = EmbeddingSpace::default_space();
        assert!(space.require_dims(384).is_ok());
        let err = space.require_dims(1536).unwrap_err();
        assert_eq!(err.code, ErrorCode::OutOfRange);
        assert!(err.message.contains("never truncated or padded"), "{err}");
    }

    #[test]
    fn require_model_fails_closed_on_mismatch() {
        let space = EmbeddingSpace::default_space();
        assert!(space.require_model(DEFAULT_EMBEDDING_MODEL).is_ok());
        let err = space
            .require_model("openai/text-embedding-3-small")
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        assert!(
            err.message.contains("does not match the active space"),
            "{err}"
        );
    }
}

#[cfg(test)]
#[path = "embedding_space_calibration_tests.rs"]
mod embedding_space_calibration_tests;

#[cfg(test)]
#[path = "embedding_space_pg_space_tests.rs"]
pub(crate) mod embedding_space_pg_space_tests;
