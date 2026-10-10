// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the fetch-gate tests of hybrid retrieval:
//! every candidate id an arm or the graph expander produces must pass the same
//! current-belief check before it can enter the page.
//!
//! ## Owns
//! - The closed-row fetch-gate regression test
//!
//! ## Does not own
//! - The fetch loop implementation (`hybrid_fetch`) or the arm behavior
//!   (`hybrid_legs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on fetch-time visibility.

use crate::fixtures_tests as fx;
use crate::types::RetrieveRequest;
use clio_store::{CorrectionSpec, Store};
use std::sync::Arc;

#[test]
fn graph_expansion_never_readmits_a_closed_row() {
    let store = fx::store();
    let seed = fx::item("seed", "bank", "alpha");
    fx::put_lexical(&store, &seed, "alpha");

    let prior = fx::item("prior", "bank", "closed belief");
    store.create_memory_item(&prior, "agent").expect("prior");
    let mut corrected = prior.clone();
    corrected.id = "live".into();
    corrected.gist = Some("current belief".into());
    corrected.tx_from = "2026-02-01T00:00:00Z".into();
    corrected.valid_from = "2026-02-01T00:00:00Z".into();
    corrected.created_at = "2026-02-01T00:00:00Z".into();
    corrected.updated_at = "2026-02-01T00:00:00Z".into();
    store
        .correct_item(&CorrectionSpec {
            bank_id: "bank".into(),
            prior_item_id: "prior".into(),
            new_item: corrected,
            reason: "belief changed".into(),
            actor: "agent".into(),
            timestamp: "2026-02-01T00:00:00Z".into(),
        })
        .expect("correct");

    // The expander reaches the closed prior, which neither search arm can: only
    // the fetch gate stands between that id and the returned page.
    let mut retriever = fx::retriever(&store);
    retriever.set_expander(Arc::new(fx::StaticExpander {
        ids: vec!["prior".into()],
    }));
    let mut req = RetrieveRequest::new("bank", "alpha");
    req.expand_graph = true;
    let out = retriever.retrieve(&req).expect("retrieve");
    assert!(
        out.hits.iter().any(|hit| hit.hit.item_id == "seed"),
        "the seed is still returned: {:?}",
        out.hits
    );
    assert!(
        !out.hits.iter().any(|hit| hit.hit.item_id == "prior"),
        "a closed row must not re-enter through graph expansion: {:?}",
        out.hits
    );
}
