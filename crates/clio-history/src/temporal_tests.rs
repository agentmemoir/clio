// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `temporal_history` façade tests:
//! triple/preference/belief trajectories, point-in-time filtering, target
//! parsing, and bounded limits.
//!
//! ## Owns
//! - Façade dispatch, malformed-target errors, and as_of semantics
//!
//! ## Does not own
//! - Underlying store SQL (see `clio-store`) or belief/continuous persistence
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::temporal::{TemporalTargetKind, parse_target};
use crate::temporal_history;
use crate::test_support::{
    T0, T1, T2, seed_belief, seed_belief_with_context, seed_preference,
    seed_preference_with_context, seed_triple, setup, temporal_args,
};
use clio_types::{ErrorCode, HistoryPolicy, TimeAxis};

#[test]
fn parses_targets_and_rejects_malformed() {
    let (kind, head, tail) = parse_target("triple:Alice:lives_in").unwrap();
    assert_eq!(kind, TemporalTargetKind::Triple);
    assert_eq!(head, "Alice");
    assert_eq!(tail, "lives_in");
    assert_eq!(
        parse_target("preference:brevity").unwrap().0,
        TemporalTargetKind::Preference
    );
    assert_eq!(
        parse_target("belief:some-id").unwrap().0,
        TemporalTargetKind::Belief
    );
    assert_eq!(
        parse_target("nope:x").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        parse_target("triple:only-subject").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        parse_target("no-colon").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        parse_target("preference:").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        parse_target("triple:Alice:").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn t08_triple_trajectory_and_point_in_time() {
    let setup = setup();
    let policy = HistoryPolicy::default();
    seed_triple(&setup, "Alice", "lives_in", "Boston", T0, T0);
    seed_triple(&setup, "Alice", "lives_in", "New York", T1, T1);

    let full = temporal_history(&temporal_args(
        &setup,
        &policy,
        "triple:Alice:lives_in",
        None,
        TimeAxis::Valid,
    ))
    .unwrap();
    assert_eq!(full.kind(), TemporalTargetKind::Triple);
    let crate::TemporalHistoryOutcome::Triple { entries, .. } = full else {
        panic!("expected triple");
    };
    assert_eq!(entries.len(), 2);

    let as_of = temporal_history(&temporal_args(
        &setup,
        &policy,
        "triple:Alice:lives_in",
        Some(T0),
        TimeAxis::Valid,
    ))
    .unwrap();
    assert_eq!(as_of.len(), 1);
}

#[test]
fn t08b_triple_as_of_keeps_current_version_beyond_limit() {
    // Regression: the point-in-time predicate must run before LIMIT so a
    // query "as of T2" still returns the version current at T2 even when more
    // versions exist than the limit.
    let setup = setup();
    let policy = HistoryPolicy::new(120, 0.5, 2, 2).unwrap();
    seed_triple(&setup, "Alice", "lives_in", "Boston", T0, T0);
    seed_triple(&setup, "Alice", "lives_in", "New York", T1, T1);
    seed_triple(&setup, "Alice", "lives_in", "Berlin", T2, T2);

    let mut args = temporal_args(
        &setup,
        &policy,
        "triple:Alice:lives_in",
        Some(T2),
        TimeAxis::Valid,
    );
    args.limit = Some(usize::MAX);
    let out = temporal_history(&args).unwrap();
    let crate::TemporalHistoryOutcome::Triple { entries, .. } = out else {
        panic!("expected triple");
    };
    assert_eq!(entries.len(), 2);
    assert!(
        entries.iter().any(|e| e.object == "Berlin"),
        "version current at as_of must survive the limit, got {entries:?}"
    );
}

#[test]
fn t09_preference_series_and_as_of() {
    let setup = setup();
    let policy = HistoryPolicy::default();
    seed_preference(&setup, "brevity_affinity", &[(0.2, T0), (0.8, T1)]);

    let full = temporal_history(&temporal_args(
        &setup,
        &policy,
        "preference:brevity_affinity",
        None,
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Preference { points, .. } = full else {
        panic!("expected preference");
    };
    assert_eq!(points.len(), 2);
    assert!(points[0].state <= points[1].state);

    let as_of = temporal_history(&temporal_args(
        &setup,
        &policy,
        "preference:brevity_affinity",
        Some(T0),
        TimeAxis::Valid,
    ))
    .unwrap();
    assert_eq!(as_of.len(), 1);
}

#[test]
fn t09c_preference_context_is_effective_from_its_introduction() {
    // Updated from the previously-leaking behavior: the stored preference
    // context used to ride every point of the series; it must now appear only
    // from the point where it became effective (the introducing write's `as_of`)
    // onward. regression: the context is supplied at T1, so a read whose
    // `as_of` excludes the T1 point must not leak it onto the T0 point.
    let setup = setup();
    let policy = HistoryPolicy::default();
    seed_preference_with_context(
        &setup,
        "brevity_affinity",
        &[(0.2, T0), (0.8, T1)],
        Some("kept short after feedback"),
    );

    // as_of = None: the pre-context point carries none, the effective point does.
    let full = temporal_history(&temporal_args(
        &setup,
        &policy,
        "preference:brevity_affinity",
        None,
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Preference { points, .. } = full else {
        panic!("expected preference");
    };
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].as_of, T0);
    assert!(
        points[0].context.is_none(),
        "a point before the context was introduced must not carry it: {points:?}"
    );
    assert_eq!(points[1].as_of, T1);
    assert_eq!(
        points[1].context.as_deref(),
        Some("kept short after feedback")
    );
    let body = serde_json::to_string(&points[0]).expect("serialize pre-context point");
    assert!(
        !body.contains("\"context\""),
        "the pre-context point omits the key: {body}"
    );
    let body = serde_json::to_string(&points[1]).expect("serialize effective point");
    assert!(
        body.contains("\"context\":\"kept short after feedback\""),
        "the effective point carries the key: {body}"
    );

    // The reviewer's shape: an as_of that excludes the later point returns only
    // the earlier point, with no context.
    let as_of = temporal_history(&temporal_args(
        &setup,
        &policy,
        "preference:brevity_affinity",
        Some(T0),
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Preference { points, .. } = as_of else {
        panic!("expected preference");
    };
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].as_of, T0);
    assert!(
        points[0].context.is_none(),
        "a point-in-time read before the context existed must not leak it: {points:?}"
    );
}

#[test]
fn t09d_preference_series_omits_context_when_none() {
    let setup = setup();
    let policy = HistoryPolicy::default();
    seed_preference(&setup, "brevity_affinity", &[(0.2, T0)]);

    let out = temporal_history(&temporal_args(
        &setup,
        &policy,
        "preference:brevity_affinity",
        None,
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Preference { points, .. } = out else {
        panic!("expected preference");
    };
    assert_eq!(points.len(), 1);
    assert!(points[0].context.is_none());
    let body = serde_json::to_string(&points).expect("serialize points");
    assert!(
        !body.contains("\"context\""),
        "no-context points must omit the key: {body}"
    );
}

#[test]
fn t10_belief_trajectory_by_id_and_proposition() {
    let setup = setup();
    let policy = HistoryPolicy::default();
    let belief_id = seed_belief(&setup, "user likes coffee", &[(0.3, T0), (0.7, T1)]);

    let by_id = temporal_history(&temporal_args(
        &setup,
        &policy,
        &format!("belief:{belief_id}"),
        None,
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Belief {
        belief_id: id,
        proposition,
        entries,
        ..
    } = by_id
    else {
        panic!("expected belief");
    };
    assert_eq!(id, belief_id);
    assert_eq!(proposition, "user likes coffee");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].confidence, 0.7);

    let by_prop = temporal_history(&temporal_args(
        &setup,
        &policy,
        "belief:user likes coffee",
        Some(T0),
        TimeAxis::Valid,
    ))
    .unwrap();
    assert_eq!(by_prop.len(), 1);
}

#[test]
fn t10b_belief_trajectory_surfaces_belief_context() {
    // The belief's stored context must ride the trajectory outcome.
    // Updated from the previously-leaking behavior: the context is surfaced only
    // from the belief's `created_at` (T0) onward, so an earlier `as_of` read no
    // longer carries it.
    let setup = setup();
    let policy = HistoryPolicy::default();
    let belief_id = seed_belief_with_context(
        &setup,
        "user likes coffee",
        &[(0.3, T0)],
        Some("from onboarding interview"),
    );

    let out = temporal_history(&temporal_args(
        &setup,
        &policy,
        &format!("belief:{belief_id}"),
        None,
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Belief {
        context, entries, ..
    } = out
    else {
        panic!("expected belief");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(context.as_deref(), Some("from onboarding interview"));

    // Regression: a read before the belief's creation must not surface its
    // context.
    let out = temporal_history(&temporal_args(
        &setup,
        &policy,
        &format!("belief:{belief_id}"),
        Some("2020-01-01"),
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Belief { context, .. } = out else {
        panic!("expected belief");
    };
    assert!(
        context.is_none(),
        "a pre-creation point-in-time read must not leak the belief context"
    );

    // A belief written without context carries none.
    let plain = seed_belief(&setup, "user likes tea", &[(0.5, T0)]);
    let out = temporal_history(&temporal_args(
        &setup,
        &policy,
        &format!("belief:{plain}"),
        None,
        TimeAxis::Valid,
    ))
    .unwrap();
    let crate::TemporalHistoryOutcome::Belief { context, .. } = out else {
        panic!("expected belief");
    };
    assert!(context.is_none());
}

#[test]
fn unknown_belief_and_bad_target_error() {
    let setup = setup();
    let policy = HistoryPolicy::default();
    let err = temporal_history(&temporal_args(
        &setup,
        &policy,
        "belief:missing",
        None,
        TimeAxis::Valid,
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = temporal_history(&temporal_args(
        &setup,
        &policy,
        "nonsense:x",
        None,
        TimeAxis::Valid,
    ))
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err =
        temporal_history(&temporal_args(&setup, &policy, "", None, TimeAxis::Valid)).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn limits_are_capped() {
    let setup = setup();
    let policy = HistoryPolicy::new(120, 0.5, 2, 2).unwrap();
    for i in 0..3 {
        let as_of = format!("2026-06-0{}", i + 1);
        seed_triple(&setup, "Bob", "knows", "Rust", &as_of, &as_of);
    }
    let mut args = temporal_args(
        &setup,
        &policy,
        "triple:Bob:knows",
        None,
        TimeAxis::Transaction,
    );
    args.limit = Some(usize::MAX);
    let out = temporal_history(&args).unwrap();
    assert_eq!(out.len(), 2);
    assert!(!out.is_empty());
}

#[path = "temporal_tests_belief_context_tests.rs"]
mod temporal_belief_context_tests;
