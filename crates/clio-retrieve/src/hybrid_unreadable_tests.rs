// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for how the hybrid retriever reacts to a stored
//! item whose content cannot be opened.
//!
//! ## Owns
//! - The skippable-code table: each code that must drop a candidate, not abort
//! - The fail-closed half: a backend failure must still abort the query
//!
//! ## Does not own
//! - Which damage produces which code (`clio-store`'s `content_read_damage_tests`)
//! - Retrieval scoring, dedup, or context composition
//!
//! # Why these are split out
//!
//! `hybrid_tests.rs` was already near the 450-line cap. These cases are about one
//! behaviour — the skip-versus-fail decision — so they live together here rather
//! than being split across a file that tests something else.
//!
//! # Why every code is driven, not just one
//!
//! The shredded-DEK case in `hybrid_tests` covers `ErasedSubject`. When #17 added
//! `UnreadableContent` for the other damage shapes, this reader kept a
//! hand-written two-code list and silently diverged from the write side, so those
//! shapes aborted the whole query. Driving every skippable code here is what stops
//! that from drifting back: a code that stops being skippable fails a test.
//!
//! The codes are driven through a faulty provider rather than by editing the
//! database, because what matters here is the reader's reaction to a code, not
//! which layer produced it. Which damage produces which code is pinned against
//! both backends in `clio-store`.

use crate::fixtures_tests as fx;
use crate::types::RetrieveRequest;
use clio_types::ErrorCode;

fn request(bank: &str, query: &str) -> RetrieveRequest {
    RetrieveRequest::new(bank, query)
}

fn ids(out: &crate::types::RetrieveOutcome) -> Vec<String> {
    out.hits.iter().map(|h| h.hit.item_id.clone()).collect()
}

/// Every skippable content code drops the candidate and keeps the query alive.
#[test]
fn every_skippable_content_code_skips_the_candidate_with_a_warning() {
    for code in [
        ErrorCode::UnreadableContent,
        ErrorCode::Forbidden,
        ErrorCode::ErasedSubject,
    ] {
        let (store, _kms) = fx::store_faulty_for("subj-b", code);
        let mut readable = fx::item("A", "bank", "alpha readable");
        readable.subject_id = "subj-a".into();
        let mut damaged = fx::item("B", "bank", "alpha damaged");
        damaged.subject_id = "subj-b".into();
        fx::put_lexical(&store, &readable, "alpha");
        fx::put_lexical(&store, &damaged, "alpha");

        // The damaged row is stored and lexically reachable, so the retriever
        // really tries to open it rather than never selecting it.
        let out = fx::retriever(&store)
            .retrieve(&request("bank", "alpha"))
            .unwrap_or_else(|e| panic!("{code} must not abort the query: {e}"));
        assert!(
            ids(&out).contains(&"A".to_owned()),
            "{code}: the readable hit must still return, got {:?}",
            ids(&out)
        );
        assert!(
            !ids(&out).contains(&"B".to_owned()),
            "{code}: the unreadable hit must be skipped, got {:?}",
            ids(&out)
        );
        assert!(
            out.warnings.iter().any(|w| w.contains("unreadable")),
            "{code}: expected a warning, got {:?}",
            out.warnings
        );
    }
}

/// A backend failure is not skippable: the query must fail rather than answer from
/// a store that is actually broken.
#[test]
fn a_backend_failure_still_aborts_the_query() {
    let (store, _kms) = fx::store_faulty_for("subj-b", ErrorCode::Internal);
    let mut readable = fx::item("A", "bank", "alpha readable");
    readable.subject_id = "subj-a".into();
    let mut broken = fx::item("B", "bank", "alpha broken");
    broken.subject_id = "subj-b".into();
    fx::put_lexical(&store, &readable, "alpha");
    fx::put_lexical(&store, &broken, "alpha");

    assert!(
        fx::retriever(&store)
            .retrieve(&request("bank", "alpha"))
            .is_err(),
        "a broken backend must not be answered as a skip"
    );
}
