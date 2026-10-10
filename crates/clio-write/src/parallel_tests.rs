// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use crate::chunk::{Chunk, Turn};
use crate::extract::{ExtractCandidate, ExtractRequest, Extractor};
use crate::parallel::{extract_chunks_parallel, map_bounded};
use crate::schema::{FieldSpec, default_event_fields};
use crate::verify::VerifyConfig;
use clio_types::{AmError, ErrorCode};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct SlowFixture {
    delay: Duration,
    active: AtomicUsize,
    max_active: AtomicUsize,
    by_source: Mutex<std::collections::HashMap<String, ExtractCandidate>>,
}

impl SlowFixture {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            by_source: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn insert(&self, source: &str, cand: ExtractCandidate) {
        self.by_source
            .lock()
            .unwrap()
            .insert(source.to_owned(), cand);
    }
}

impl Extractor for SlowFixture {
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
        let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(n, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.by_source
            .lock()
            .unwrap()
            .get(&req.source_text)
            .cloned()
            .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "missing fixture"))
    }
}

#[test]
fn t01_chunks_run_concurrently_under_cap() {
    let delay = Duration::from_millis(80);
    let fx = SlowFixture::new(delay);
    for i in 0..4 {
        let text = format!("Ada turn {i}");
        fx.insert(
            &text,
            ExtractCandidate {
                snapshot: Some(json!({"entity": "Ada"})),
                gist: Some("g".into()),
                source_ref: format!("t{i}"),
                diagnostic: None,
            },
        );
    }
    let turns: Vec<_> = (0..4)
        .map(|i| Turn::text(format!("Ada turn {i}")))
        .collect();
    let chunks: Vec<Chunk> = turns
        .iter()
        .enumerate()
        .map(|(i, t)| Chunk {
            chunk_id: format!("c{i}"),
            source_text: t.source_text.clone(),
            tool_payload: None,
            turn_indices: vec![i],
        })
        .collect();
    let fields = default_event_fields();
    let start = Instant::now();
    let agg = extract_chunks_parallel(
        &fx,
        &chunks,
        &json!({}),
        &fields,
        &VerifyConfig::default(),
        4,
        None,
    );
    let elapsed = start.elapsed();
    assert_eq!(agg.verified_count, 4);
    // The overlap claim is gated on the fixture's own concurrency gauge, not on
    // a duration. `max_active` is what the code under test actually controls: it
    // counts how many extractions were in flight at once. A wall-clock bound
    // here was a proxy for the same claim with only 25% of headroom over the
    // fully-serialised case (4 x 80ms is 320ms against a 240ms limit), so an
    // ordinary sleep overshoot on a loaded runner failed a test whose code was
    // unchanged. The duration is still checked, but against a bound loose
    // enough that host load cannot reach it, and it is printed either way.
    let max_active = fx.max_active.load(Ordering::SeqCst);
    assert!(
        max_active >= 2,
        "expected concurrent workers, max_active={max_active}"
    );
    assert!(
        max_active <= 4,
        "worker cap exceeded: max_active={max_active}"
    );
    assert!(
        elapsed < delay * 12,
        "four 80ms chunks took {elapsed:?}, far beyond even a serial run"
    );
    println!("4 chunks x {delay:?} with cap 4: max_active={max_active}, elapsed={elapsed:?}");
}

#[test]
fn map_bounded_preserves_order_and_cap() {
    let barrier = Barrier::new(2);
    let active = AtomicUsize::new(0);
    let max = AtomicUsize::new(0);
    let out = map_bounded(vec![1, 2, 3], 2, |x| {
        let n = active.fetch_add(1, Ordering::SeqCst) + 1;
        max.fetch_max(n, Ordering::SeqCst);
        if x <= 2 {
            barrier.wait();
        }
        active.fetch_sub(1, Ordering::SeqCst);
        x * 10
    });
    assert_eq!(out, vec![10, 20, 30]);
    assert!(max.load(Ordering::SeqCst) <= 2);
}

#[derive(Debug)]
struct NoopExtractor;

impl Extractor for NoopExtractor {
    fn extract(&self, _: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
        unreachable!()
    }
}

#[test]
fn supplied_context_reaches_every_chunk_request() {
    #[derive(Debug)]
    struct Recording {
        contexts: Mutex<Vec<Option<String>>>,
    }
    impl Extractor for Recording {
        fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
            self.contexts.lock().unwrap().push(req.context.clone());
            Ok(ExtractCandidate {
                snapshot: Some(json!({"entity": "Ada"})),
                gist: Some("g".into()),
                source_ref: "r".into(),
                diagnostic: None,
            })
        }
    }
    let source = "Ada likes tea";
    let chunks: Vec<Chunk> = (0..2)
        .map(|i| Chunk {
            chunk_id: format!("c{i}"),
            source_text: source.to_owned(),
            tool_payload: None,
            turn_indices: vec![i],
        })
        .collect();
    let fields = default_event_fields();
    let rec = Recording {
        contexts: Mutex::new(vec![]),
    };
    let agg = extract_chunks_parallel(
        &rec,
        &chunks,
        &json!({}),
        &fields,
        &VerifyConfig::default(),
        2,
        Some("team chat"),
    );
    assert_eq!(agg.verified_count, 2, "{agg:?}");
    assert_eq!(
        rec.contexts.lock().unwrap().as_slice(),
        [Some("team chat".to_owned()), Some("team chat".to_owned())]
    );
}

#[test]
fn invalid_context_rejects_every_chunk_without_extractor_calls() {
    #[derive(Debug)]
    struct MustNotRun {
        calls: std::sync::atomic::AtomicUsize,
    }
    impl Extractor for MustNotRun {
        fn extract(&self, _req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ExtractCandidate {
                snapshot: None,
                gist: Some("g".into()),
                source_ref: "r".into(),
                diagnostic: None,
            })
        }
    }
    let chunks: Vec<Chunk> = (0..2)
        .map(|i| Chunk {
            chunk_id: format!("c{i}"),
            source_text: "Ada likes tea".to_owned(),
            tool_payload: None,
            turn_indices: vec![i],
        })
        .collect();
    let fields = default_event_fields();
    let ex = MustNotRun {
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let oversized = "x".repeat(4097);
    let agg = extract_chunks_parallel(
        &ex,
        &chunks,
        &json!({}),
        &fields,
        &VerifyConfig::default(),
        2,
        Some(oversized.as_str()),
    );
    assert_eq!(agg.verified_count, 0, "{agg:?}");
    assert_eq!(agg.error_count, 2, "{agg:?}");
    assert!(agg.results.iter().all(|r| r.candidate.is_none()), "{agg:?}");
    assert_eq!(ex.calls.load(Ordering::SeqCst), 0, "extractor must not run");
}

#[test]
fn empty_parallel_aggregate() {
    let fields: [FieldSpec; 0] = [];
    let agg = extract_chunks_parallel(
        &NoopExtractor,
        &[],
        &json!({}),
        &fields,
        &VerifyConfig::default(),
        4,
        None,
    );
    assert!(agg.results.is_empty(), "results: {agg:?}");
}
