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
fn single_batch_at_and_under_limit_preserves_provider_order() {
    for batch_size in [3, 4] {
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
        let expected: Vec<String> = ids(&fused).into_iter().rev().map(str::to_owned).collect();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn Reranker> = Arc::new(BatchReranker {
            batch_size,
            calls: Arc::clone(&calls),
            fail_after: None,
        });
        let mut warnings = Vec::new();
        let (returned, reranked) =
            apply_rerank(Some(&provider), 60, "query", &items, fused, &mut warnings);
        assert!(reranked);
        assert!(warnings.is_empty());
        assert_eq!(*calls.lock().expect("calls lock"), vec![3]);
        assert_eq!(ids(&returned), expected);
    }
}

#[test]
fn http_batches_share_the_deadline_through_the_orchestrator() {
    use crate::rerank::HttpReranker;
    use crate::rerank_seam::read_request_body;
    use std::io::Write;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    listener.set_nonblocking(true).expect("nonblocking");
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        for _ in 0..2 {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("timeout");
            read_request_body(&mut stream);
            std::thread::sleep(Duration::from_millis(200));
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]");
        }
    });
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
    let expected: Vec<String> = ids(&fused).into_iter().map(str::to_owned).collect();
    let mut client = HttpReranker::new(url, None).expect("client");
    client.set_max_batch(2);
    client.set_timeout(Duration::from_millis(350));
    let provider: Arc<dyn Reranker> = Arc::new(client);
    let mut warnings = Vec::new();
    let (returned, reranked) =
        apply_rerank(Some(&provider), 60, "query", &items, fused, &mut warnings);
    server.join().expect("server join");
    assert!(!reranked);
    assert_eq!(ids(&returned), expected);
    assert!(
        warnings.iter().any(|w| w.contains("chunk 2/2 (docs 2..3)")),
        "{warnings:?}"
    );
}

#[test]
fn http_batch_failure_reports_shortlist_wide_positions() {
    use crate::rerank::HttpReranker;
    use crate::rerank_seam::batch_server;

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
    let expected: Vec<String> = ids(&fused).into_iter().map(str::to_owned).collect();
    let ok = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]";
    let failure =
        "HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
    let (url, handle) = batch_server(vec![ok.into(), failure.into()]);
    let mut client = HttpReranker::new(url, None).expect("client");
    client.set_max_batch(2);
    let provider: Arc<dyn Reranker> = Arc::new(client);
    let mut warnings = Vec::new();
    let (returned, reranked) =
        apply_rerank(Some(&provider), 60, "query", &items, fused, &mut warnings);
    handle.join().expect("server join");
    assert!(!reranked);
    assert_eq!(ids(&returned), expected);
    assert!(
        warnings.iter().any(|w| w.contains("chunk 2/2 (docs 2..3)")),
        "{warnings:?}"
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
