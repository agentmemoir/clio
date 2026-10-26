// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::rerank::Reranker;
use crate::rerank_parse::RerankedDoc;
use clio_types::AmError;
use std::sync::{Arc, Mutex};

struct BatchReranker {
    batch_size: usize,
    calls: Arc<Mutex<Vec<usize>>>,
    fail_after: Option<usize>,
}

impl Reranker for BatchReranker {
    fn rerank(
        &self,
        _query: &str,
        documents: &[String],
        _top_k: usize,
    ) -> Result<Vec<RerankedDoc>, AmError> {
        let mut calls = self.calls.lock().expect("calls lock");
        let call = calls.len();
        calls.push(documents.len());
        if self.fail_after.is_some_and(|limit| call >= limit) {
            return Err(AmError::new(
                clio_types::ErrorCode::Internal,
                "second rerank batch failed",
            ));
        }
        let base = call as f64 * 0.8;
        Ok((0..documents.len())
            .rev()
            .map(|index| RerankedDoc {
                index,
                score: Some(base + index as f64 * 0.1),
            })
            .collect())
    }

    fn max_batch_size(&self) -> usize {
        self.batch_size
    }
}

#[test]
fn rerank_splits_shortlist_and_merges_batch_scores() {
    let items = items_at_now();
    let fused = fuse_candidates(
        &weights_with(0.25),
        DEFAULT_RRF_K,
        &fixture_legs().0,
        &fixture_legs().1,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Reranker> = Arc::new(BatchReranker {
        batch_size: 2,
        calls: Arc::clone(&calls),
        fail_after: None,
    });
    let mut warnings = Vec::new();
    let (returned, reranked) =
        apply_rerank(Some(&provider), 60, "query", &items, fused, &mut warnings);

    assert!(reranked);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(*calls.lock().expect("calls lock"), vec![2, 1]);
    assert_eq!(ids(&returned), vec!["C", "A", "B"]);
    assert_eq!(
        returned
            .iter()
            .map(|hit| hit.rerank_score)
            .collect::<Vec<_>>(),
        vec![Some(0.8), Some(0.1), Some(0.0)]
    );
}

#[test]
fn a_late_rerank_batch_failure_keeps_the_fused_order() {
    let items = items_at_now();
    let fused = fuse_candidates(
        &weights_with(0.25),
        DEFAULT_RRF_K,
        &fixture_legs().0,
        &fixture_legs().1,
        &[],
        &items,
        &BTreeSet::new(),
        NO_CURVE,
        TrustRatios::default(),
    );
    let expected = ids(&fused);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Reranker> = Arc::new(BatchReranker {
        batch_size: 2,
        calls,
        fail_after: Some(1),
    });
    let mut warnings = Vec::new();
    let (returned, reranked) = apply_rerank(
        Some(&provider),
        60,
        "query",
        &items,
        fused.clone(),
        &mut warnings,
    );

    assert!(!reranked);
    assert_eq!(ids(&returned), expected);
    assert_eq!(
        returned
            .iter()
            .map(|hit| hit.rerank_score)
            .collect::<Vec<_>>(),
        vec![None, None, None]
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("second rerank batch failed"))
    );
}
