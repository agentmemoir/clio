// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `dense_search` / `lexical_search`
//! primitive facades consumed by hybrid retrieval.
//!
//! ## Owns
//! - Typed search argument shapes (`DenseSearchArgs` / `LexicalSearchArgs`)
//! - Fail-closed bank/domain/limit validation before backend dispatch
//!
//! ## Does not own
//! - Backend search SQL (`clio_store::IndexStore`)
//! - Hybrid fusion, rerank, or the intent gate
//!
//! ## Boundary
//!
//! Changes to the primitive signatures or validation should normally require
//! changes here. Changes to backend ranking SQL should not require changes here.

use clio_store::{IndexStore, KnnHit, SearchHit};
use clio_types::{AmError, ErrorCode, SearchDomain};

/// Arguments for one dense KNN query (`query_vector, domains?, limit?, bank?`).
#[derive(Debug, Clone)]
pub struct DenseSearchArgs<'a> {
    /// Bank isolation key (required; empty bank fails closed).
    pub bank: &'a str,
    /// Query vector; dimensions must match the store schema.
    pub query: &'a [f32],
    /// Domain filters parsed from wire strings (`[]` = no filter).
    pub domains: Vec<String>,
    /// Max hits (0 fails closed; callers pick their own default).
    pub limit: usize,
}

/// Arguments for one lexical search (`query_text, domains?, limit?, bank?`).
#[derive(Debug, Clone)]
pub struct LexicalSearchArgs<'a> {
    /// Bank isolation key (required; empty bank fails closed).
    pub bank: &'a str,
    /// Raw query text (tokens are escaped per backend).
    pub query: &'a str,
    /// Domain filters parsed from wire strings (`[]` = no filter).
    pub domains: Vec<String>,
    /// Max hits.
    pub limit: usize,
}

/// Dense cosine KNN scoped to one bank (lower `distance` = closer).
///
/// # Errors
/// Missing bank, unknown domain, zero limit, dimension mismatch, storage.
pub fn dense_search(
    index: &dyn IndexStore,
    args: &DenseSearchArgs<'_>,
) -> Result<Vec<KnnHit>, AmError> {
    let domains = parse_domains(&args.domains)?;
    if args.limit == 0 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "dense_search limit must be positive",
        ));
    }
    index.dense_search(args.bank, args.query, &domains, args.limit)
}

/// BM25-style lexical search scoped to one bank (higher `score` = better).
///
/// # Errors
/// Missing bank, empty/unknown query, unknown domain, or storage failures.
pub fn lexical_search(
    index: &dyn IndexStore,
    args: &LexicalSearchArgs<'_>,
) -> Result<Vec<SearchHit>, AmError> {
    let domains = parse_domains(&args.domains)?;
    if args.limit == 0 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "lexical_search limit must be positive",
        ));
    }
    index.lexical_search(args.bank, args.query, &domains, args.limit)
}

fn parse_domains(raw: &[String]) -> Result<Vec<SearchDomain>, AmError> {
    SearchDomain::parse_all(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed_test_support::fake_vector;
    use clio_store::{EMBEDDING_DIMS, SqliteStore};

    #[test]
    fn happy_paths_delegate_to_backend() {
        let kms: std::sync::Arc<dyn clio_store::DekProvider> =
            std::sync::Arc::new(clio_store::LocalDevKms::new());
        let store = SqliteStore::open_in_memory(kms).unwrap();
        let hits = dense_search(
            &store,
            &DenseSearchArgs {
                bank: "s",
                query: &fake_vector("q", EMBEDDING_DIMS),
                domains: vec!["semantic".into()],
                limit: 5,
            },
        )
        .unwrap();
        assert!(hits.is_empty(), "empty store yields no hits");
        let hits = lexical_search(
            &store,
            &LexicalSearchArgs {
                bank: "s",
                query: "exact token",
                domains: vec!["episodic".into(), "persona".into()],
                limit: 3,
            },
        )
        .unwrap();
        assert!(hits.is_empty(), "{hits:?}");
    }
}
