// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for generating the width-parameterized vector
//! table and index DDL for both storage backends.
//!
//! ## Owns
//! - `postgres_vector_ddl` / `sqlite_vector_ddl` for a resolved width
//! - The width-driven index flavour (`vector` HNSW / `halfvec` expression / none)
//! - The pgvector minimum-version check required by the `halfvec` path
//!
//! ## Does not own
//! - Space identity or width policy (`embedding_space`)
//! - Applying DDL during bootstrap or a space switch (backend modules)
//! - The static (width-independent) SQL assets under `sql/`
//!
//! ## Boundary
//!
//! Changes to generated vector DDL should normally require changes here.
//! Changes to non-vector DDL should not require changes here.
//!
//! Keep this module focused on width-parameterized vector DDL text.

use crate::embedding_space::{EmbeddingSpace, IndexMode};
use clio_types::{AmError, ErrorCode};
use std::fmt::Write as _;

/// Lowest pgvector release with `halfvec` (`>= 0.7.0`).
pub const HALFVEC_MIN_VERSION: &str = "0.7.0";
/// Same floor as a parsed `(major, minor, patch)` triple.
const HALFVEC_MIN_TRIPLE: [u32; 3] = [0, 7, 0];

/// Right-pad a column name so its type starts at column 96 (sql-guidelines).
fn col(name: &str, rest: &str) -> String {
    if name.len() + 3 >= 93 {
        format!("  {name} {rest}")
    } else {
        format!("  {name:<92} {rest}")
    }
}

/// Postgres vector-table DDL for `space`, including the width-appropriate index.
///
/// # Errors
/// Zero/oversized width (`EmbeddingSpace::new` rejects these upstream).
pub fn postgres_vector_ddl(space: &EmbeddingSpace) -> Result<String, AmError> {
    let dims = space.dims;
    if dims == 0 || dims > crate::embedding_space::MAX_VECTOR_DIMS {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("embedding width {dims} is outside 1..=16000"),
        ));
    }
    let mut ddl = String::new();
    ddl.push_str("CREATE TABLE IF NOT EXISTS item_embeddings\n(\n");
    ddl.push_str(&col(
        "item_id",
        "text PRIMARY KEY REFERENCES items (id) ON DELETE CASCADE",
    ));
    ddl.push_str(",\n");
    ddl.push_str(&col("bank_id", "text NOT NULL"));
    ddl.push_str(",\n");
    ddl.push_str(&col("model_id", "text NOT NULL"));
    ddl.push_str(",\n");
    ddl.push_str(&col("dims", "integer NOT NULL"));
    ddl.push_str(",\n");
    ddl.push_str(&col("embedding", &format!("vector({dims}) NOT NULL")));
    ddl.push_str(",\n");
    ddl.push_str(&col("created_at", "text NOT NULL"));
    ddl.push_str(",\n");
    ddl.push_str(&col("updated_at", "text NOT NULL"));
    ddl.push_str("\n);\n");
    ddl.push_str(
        "CREATE INDEX IF NOT EXISTS item_embeddings_bank_id_inx\nON item_embeddings (bank_id);\n",
    );
    match space.index_mode() {
        IndexMode::AnnVector => ddl.push_str(
            "CREATE INDEX IF NOT EXISTS item_embeddings_embedding_hnsw_inx\nON item_embeddings\nUSING hnsw (embedding vector_cosine_ops);\n",
        ),
        IndexMode::AnnHalfvec => {
            let _ = write!(
                ddl,
                "CREATE INDEX IF NOT EXISTS item_embeddings_embedding_hnsw_inx\nON item_embeddings\nUSING hnsw ((embedding::halfvec({dims})) halfvec_cosine_ops);\n"
            );
        }
        IndexMode::ExactScan => {}
    }
    Ok(ddl)
}

/// SQLite `vec0` DDL for `space` (exact KNN regardless of width).
///
/// # Errors
/// Zero/oversized width.
pub fn sqlite_vector_ddl(space: &EmbeddingSpace) -> Result<String, AmError> {
    let dims = space.dims;
    if dims == 0 || dims > crate::embedding_space::MAX_VECTOR_DIMS {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("embedding width {dims} is outside 1..=16000"),
        ));
    }
    let mut ddl =
        String::from("CREATE VIRTUAL TABLE IF NOT EXISTS item_embeddings USING vec0\n(\n");
    ddl.push_str(&col("item_id", "text PRIMARY KEY"));
    ddl.push_str(",\n");
    ddl.push_str(&col(
        "embedding",
        &format!("float[{dims}] distance_metric = cosine"),
    ));
    ddl.push_str(",\n");
    ddl.push_str(&col("+bank_id", "text"));
    ddl.push_str(",\n");
    ddl.push_str(&col("+model_id", "text"));
    ddl.push_str(",\n");
    ddl.push_str(&col("+created_at", "text"));
    ddl.push_str(",\n");
    ddl.push_str(&col("+updated_at", "text"));
    ddl.push_str("\n);\n");
    Ok(ddl)
}

/// True when a pgvector `extversion` string meets [`HALFVEC_MIN_VERSION`].
///
/// A trailing pre-release/build suffix is ignored (`0.8.6-beta` → `0.8.6`);
/// an unparsable version fails closed.
#[must_use]
pub fn pgvector_supports_halfvec(extversion: &str) -> bool {
    parse_version(extversion).is_some_and(|v| v >= HALFVEC_MIN_TRIPLE)
}

fn parse_version(raw: &str) -> Option<[u32; 3]> {
    let core = raw.split(['-', '+']).next().unwrap_or_default().trim();
    let mut out = [0_u32; 3];
    let mut parts = core.split('.');
    for slot in &mut out {
        *slot = parts.next()?.trim().parse::<u32>().ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_ddl_by_width() {
        let plain = postgres_vector_ddl(&EmbeddingSpace::new("m", 384).unwrap()).unwrap();
        assert!(plain.contains("vector(384) NOT NULL"), "{plain}");
        assert!(
            plain.contains("USING hnsw (embedding vector_cosine_ops)"),
            "{plain}"
        );
        assert!(!plain.contains("halfvec"), "{plain}");
        assert!(!plain.contains("CHECK"), "{plain}");

        let half = postgres_vector_ddl(&EmbeddingSpace::new("m", 3072).unwrap()).unwrap();
        assert!(half.contains("vector(3072) NOT NULL"), "{half}");
        assert!(
            half.contains("USING hnsw ((embedding::halfvec(3072)) halfvec_cosine_ops)"),
            "{half}"
        );

        let exact = postgres_vector_ddl(&EmbeddingSpace::new("m", 4096).unwrap()).unwrap();
        assert!(exact.contains("vector(4096) NOT NULL"), "{exact}");
        assert!(!exact.contains("hnsw"), "{exact}");

        assert_eq!(
            postgres_vector_ddl(&EmbeddingSpace {
                model_id: "m".into(),
                dims: 0
            })
            .unwrap_err()
            .code,
            ErrorCode::OutOfRange
        );
        assert_eq!(
            postgres_vector_ddl(&EmbeddingSpace {
                model_id: "m".into(),
                dims: 16_001
            })
            .unwrap_err()
            .code,
            ErrorCode::OutOfRange
        );
    }

    #[test]
    fn sqlite_ddl_carries_width_and_aux_columns() {
        let ddl = sqlite_vector_ddl(&EmbeddingSpace::new("m", 1536).unwrap()).unwrap();
        assert!(ddl.contains("USING vec0"), "{ddl}");
        assert!(
            ddl.contains("float[1536] distance_metric = cosine"),
            "{ddl}"
        );
        assert!(ddl.contains("+model_id"), "{ddl}");
        assert_eq!(
            sqlite_vector_ddl(&EmbeddingSpace {
                model_id: "m".into(),
                dims: 0
            })
            .unwrap_err()
            .code,
            ErrorCode::OutOfRange
        );
        assert_eq!(
            sqlite_vector_ddl(&EmbeddingSpace {
                model_id: "m".into(),
                dims: 99_999
            })
            .unwrap_err()
            .code,
            ErrorCode::OutOfRange
        );
    }

    #[test]
    fn halfvec_version_gate() {
        assert!(pgvector_supports_halfvec("0.8.6"));
        assert!(pgvector_supports_halfvec("0.7.0"));
        assert!(pgvector_supports_halfvec("0.7.0-beta"));
        assert!(pgvector_supports_halfvec("1.0.0"));
        assert!(!pgvector_supports_halfvec("0.6.2"));
        assert!(!pgvector_supports_halfvec("0.7"));
        assert!(!pgvector_supports_halfvec("not-a-version"));
        assert!(!pgvector_supports_halfvec(""));
    }

    #[test]
    fn long_column_names_fall_back_to_single_space() {
        let wide = col(
            "a_very_long_column_name_that_reaches_past_the_alignment_width_used_by_sql_guidelines",
            "text",
        );
        assert!(wide.starts_with("  a_very_long"));
        assert!(wide.ends_with(" text"));
    }
}
