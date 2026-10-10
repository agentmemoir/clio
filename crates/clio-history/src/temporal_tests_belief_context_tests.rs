// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the belief-context effective-time tests of
//! the `temporal_history` belief branch: point-in-time reads gate on the
//! store-derived `context_as_of` stamp, not the belief's creation time.
//!
//! ## Owns
//! - `t10c_belief_context_as_of_gates_point_in_time_reads`
//!
//! ## Does not own
//! - The gate itself (`temporal`) or belief persistence (`clio-store`)
//! - The other `temporal_history` façade tests (`temporal_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::TemporalHistoryOutcome;
use crate::temporal_history;
use crate::test_support::{Setup, T0, T1, T2, seed_belief_with_context, setup, temporal_args};
use clio_store::BeliefStore as _;
use clio_types::{HistoryPolicy, TimeAxis};

fn context_at(setup: &Setup, belief_id: &str, as_of: Option<&str>) -> Option<String> {
    let policy = HistoryPolicy::default();
    let out = temporal_history(&temporal_args(
        setup,
        &policy,
        &format!("belief:{belief_id}"),
        as_of,
        TimeAxis::Valid,
    ))
    .expect("temporal read");
    let TemporalHistoryOutcome::Belief { context, .. } = out else {
        panic!("expected belief");
    };
    context
}

/// the authorized correction makes belief context mutable, so
/// the temporal gate keys on the store-derived `context_as_of` stamp, not on
/// the belief's creation time. Honest semantics: a point-in-time read between
/// the original introduction and a later correction returns NO context — the
/// prior value is unrecoverable by design (forbids retaining it) and the
/// future-corrected context is never surfaced early.
#[test]
fn t10c_belief_context_as_of_gates_point_in_time_reads() {
    let setup = setup();
    let belief_id = seed_belief_with_context(
        &setup,
        "user eats oat milk-free cereal",
        &[(0.3, T0)],
        Some("original january context"),
    );

    // Before any correction the context is effective from `created_at` (T0).
    assert_eq!(
        context_at(&setup, &belief_id, Some(T0)).as_deref(),
        Some("original january context")
    );

    setup
        .store
        .correct_belief_context(
            setup.ctx.bank.as_str(),
            &belief_id,
            "corrected february context",
            "operator fix",
            "seeder",
        )
        .expect("correction");
    let stamp = setup
        .store
        .get_belief(setup.ctx.bank.as_str(), &belief_id)
        .expect("get")
        .expect("belief")
        .context_as_of
        .expect("the correction stamps the effective time");

    // as-of before the original introduction: no context.
    assert_eq!(context_at(&setup, &belief_id, Some("2020-01-01")), None);
    // as-of in [introduction, correction): NO context — never the corrected
    // one and never the (unretained) original.
    assert_eq!(context_at(&setup, &belief_id, Some(T0)), None);
    assert_eq!(context_at(&setup, &belief_id, Some(T1)), None);
    assert_eq!(context_at(&setup, &belief_id, Some(T2)), None);
    // as-of at/after the correction stamp: the corrected context.
    assert_eq!(
        context_at(&setup, &belief_id, Some(&stamp)).as_deref(),
        Some("corrected february context")
    );
    // as-of None: the current context.
    assert_eq!(
        context_at(&setup, &belief_id, None).as_deref(),
        Some("corrected february context")
    );
}
