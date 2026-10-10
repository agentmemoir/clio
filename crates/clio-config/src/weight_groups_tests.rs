// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the typed weight groups and of
//! the shared normalize-or-reject primitive they all apply.
//!
//! ## Owns
//! - Tests for the shape, defaults, and strictness of `RetrievalWeights`
//! - Tests for the shape, defaults, and strictness of `AdmissionWeights`
//! - Tests for `IntentGateThresholds`: its default and its JSON form
//! - Tests for the renormalize-or-reject rule in `normalize_weights`
//!
//! ## Does not own
//! - Patch merging and ranking-environment validation (see `ranking`)
//! - The weight values retrieval applies while shaping a hit (see `clio-retrieve`)
//!
//! ## Boundary
//!
//! Changes to a weight group's schema or its default should require a change
//! here. A change to how a patch is merged should not require a change here.
//!
//! Keep this module focused on the weight groups and their normalization rule.

use super::*;
use crate::ranking::RankingEnvPatch;

/// A three-weight group has exactly three weights and its default sums to 1.0.
#[test]
fn retrieval_weights_default_is_three_weights_summing_to_one() {
    let w = RetrievalWeights::default();
    let slice = w.as_slice();
    assert_eq!(slice.len(), 3, "three weights: {slice:?}");
    assert_eq!(slice[0], slice[1], "default weights are equal");
    assert_eq!(slice[1], slice[2], "default weights are equal");
    let sum: f64 = slice.iter().sum();
    assert!((sum - 1.0).abs() < 1e-9, "default sum was {sum}");
}

/// The slice helpers keep each weight in its own field, in declaration order.
#[test]
fn retrieval_weights_slice_round_trip_keeps_field_order() {
    let w = RetrievalWeights::from_slice([0.1, 0.2, 0.7]);
    assert!((w.dense - 0.1).abs() < 1e-12);
    assert!((w.lexical - 0.2).abs() < 1e-12);
    assert!((w.importance - 0.7).abs() < 1e-12);
    assert_eq!(w.as_slice(), [0.1, 0.2, 0.7]);
    let again = RetrievalWeights::from_slice(w.as_slice());
    assert_eq!(again, w);
}

/// The three declared weights parse on their own, so the group is usable.
#[test]
fn retrieval_weights_accepts_a_three_field_payload() {
    let patch: RankingEnvPatch = serde_json::from_value(serde_json::json!({
        "retrieval": {"dense": 0.5, "lexical": 0.25, "importance": 0.25}
    }))
    .expect("three weights parse");
    let w = patch.retrieval.expect("retrieval present");
    assert!((w.dense - 0.5).abs() < 1e-12);
    assert!((w.lexical - 0.25).abs() < 1e-12);
    assert!((w.importance - 0.25).abs() < 1e-12);
}

/// A weight the group no longer declares is rejected, and the error names it.
#[test]
fn retrieval_weights_rejects_a_leftover_temporal_key() {
    let err = serde_json::from_value::<RankingEnvPatch>(serde_json::json!({
        "retrieval": {
            "dense": 0.25, "lexical": 0.25, "importance": 0.25, "temporal": 0.25
        }
    }))
    .expect_err("temporal is not a retrieval weight");
    let msg = err.to_string();
    assert!(msg.contains("unknown field `temporal`"), "got: {msg}");
    assert!(
        msg.contains("expected one of `dense`, `lexical`, `importance`"),
        "got: {msg}"
    );
}

/// A stale patch that both carries the old key and drops a live one reports the
/// old key, so the reader is told what to delete first.
#[test]
fn retrieval_weights_names_the_unknown_key_before_a_missing_one() {
    let err = serde_json::from_value::<RankingEnvPatch>(serde_json::json!({
        "retrieval": {"dense": 0.25, "lexical": 0.25, "temporal": 0.25}
    }))
    .expect_err("importance is missing and temporal is unknown");
    let msg = err.to_string();
    assert!(msg.contains("unknown field `temporal`"), "got: {msg}");
    assert!(!msg.contains("missing field"), "got: {msg}");
}

/// A four-factor admission group has four weights and its default sums to 1.0.
#[test]
fn admission_weights_default_is_four_weights_summing_to_one() {
    let w = AdmissionWeights::default();
    let slice = w.as_slice();
    assert_eq!(slice.len(), 4, "four factors: {slice:?}");
    // 0.25 is the literal in the group's default, so four of them are 0.25 * 4.0 = 1.0.
    for (i, v) in slice.iter().enumerate() {
        assert!((*v - 0.25).abs() < 1e-12, "factor {i} was {v}");
    }
    let sum: f64 = slice.iter().sum();
    assert!((sum - 1.0).abs() < 1e-9, "default sum was {sum}");
}

/// The slice helpers keep each factor in its own field, in declaration order.
#[test]
fn admission_weights_slice_round_trip_keeps_field_order() {
    let values = [0.1, 0.2, 0.3, 0.4];
    let w = AdmissionWeights::from_slice(values);
    for (got, want) in w.as_slice().into_iter().zip(values) {
        assert!(
            (got - want).abs() < 1e-12,
            "round trip gave {got}, want {want}"
        );
    }
}

/// Position `n` of the slice is factor `w(n+1)`, so a reordering is visible.
#[test]
fn admission_weights_from_slice_maps_each_position_to_its_own_factor() {
    let w = AdmissionWeights::from_slice([0.1, 0.2, 0.3, 0.4]);
    assert!((w.w1 - 0.1).abs() < 1e-12, "w1 was {}", w.w1);
    assert!((w.w2 - 0.2).abs() < 1e-12, "w2 was {}", w.w2);
    assert!((w.w3 - 0.3).abs() < 1e-12, "w3 was {}", w.w3);
    assert!((w.w4 - 0.4).abs() < 1e-12, "w4 was {}", w.w4);
}

/// The four declared factors parse as a patch, in the order they are written.
#[test]
fn admission_weights_accepts_a_four_field_payload() {
    let patch: RankingEnvPatch = serde_json::from_value(serde_json::json!({
        "admission": {"w1": 0.1, "w2": 0.2, "w3": 0.3, "w4": 0.4}
    }))
    .expect("four factors parse");
    let w = patch.admission.expect("admission present");
    for (got, want) in w.as_slice().into_iter().zip([0.1, 0.2, 0.3, 0.4]) {
        assert!((got - want).abs() < 1e-12, "parsed {got}, want {want}");
    }
}

/// A patch that drops a factor is rejected, and the error names the missing one.
#[test]
fn admission_weights_rejects_a_patch_missing_a_factor() {
    let err = serde_json::from_value::<RankingEnvPatch>(serde_json::json!({
        "admission": {"w1": 0.2, "w2": 0.2, "w3": 0.6}
    }))
    .expect_err("w4 is missing");
    let msg = err.to_string();
    assert!(msg.contains("missing field `w4`"), "got: {msg}");
}

/// A patch that carries the retired recency weight is rejected, not ignored.
#[test]
fn admission_weights_rejects_a_retired_weight() {
    let err = serde_json::from_value::<RankingEnvPatch>(serde_json::json!({
        "admission": {"w1": 0.2, "w2": 0.2, "w3": 0.3, "w4": 0.2, "w5": 0.1}
    }))
    .expect_err("w5 is a retired weight");
    let msg = err.to_string();
    assert!(msg.contains("unknown field `w5`"), "got: {msg}");
}

/// The gate ships with no confidence set, so nothing skips retrieval by default.
#[test]
fn intent_gate_thresholds_default_has_no_skip_confidence() {
    assert!(IntentGateThresholds::default().skip_confidence.is_none());
}

/// A set confidence survives a JSON round trip unchanged.
#[test]
fn intent_gate_thresholds_round_trips_a_set_confidence() {
    let w = IntentGateThresholds {
        skip_confidence: Some(0.8),
    };
    let text = serde_json::to_string(&w).expect("serialise");
    let back: IntentGateThresholds = serde_json::from_str(&text).expect("parse");
    let back = back.skip_confidence.expect("the threshold survives");
    assert!((back - 0.8).abs() < 1e-12, "round trip gave {back}");
}

/// An unset confidence is written as a null and reads back as unset.
#[test]
fn intent_gate_thresholds_round_trips_an_unset_confidence() {
    let text = serde_json::to_string(&IntentGateThresholds::default()).expect("serialise");
    let back: IntentGateThresholds = serde_json::from_str(&text).expect("parse");
    assert!(back.skip_confidence.is_none(), "got {back:?}");
}

/// A payload that leaves the factor out reads as "no threshold", because an
/// `Option` field is optional in serde and the group declares no other default.
#[test]
fn intent_gate_thresholds_reads_a_payload_without_the_factor_as_unset() {
    let w: IntentGateThresholds =
        serde_json::from_value(serde_json::json!({})).expect("an absent Option is unset");
    assert!(w.skip_confidence.is_none(), "got {w:?}");
}

/// A non-unit sum is rescaled so the applied weights add up to 1.0.
#[test]
fn normalize_weights_renormalizes_a_non_unit_sum() {
    let out = normalize_weights(&[2.0, 2.0, 6.0]).expect("positive sum");
    assert!((out[0] - 0.2).abs() < 1e-12, "got {out:?}");
    assert!((out[1] - 0.2).abs() < 1e-12, "got {out:?}");
    assert!((out[2] - 0.6).abs() < 1e-12, "got {out:?}");
    let sum: f64 = out.iter().sum();
    assert!((sum - 1.0).abs() < 1e-12, "sum was {sum}");
}

/// A sum of zero has no direction to rescale towards, so it is rejected.
#[test]
fn normalize_weights_rejects_a_zero_sum() {
    let err = normalize_weights(&[0.0, 0.0, 0.0]).expect_err("zero sum");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}

/// A negative or non-finite weight has no meaning in a blend, so it is rejected.
#[test]
fn normalize_weights_rejects_a_negative_or_non_finite_weight() {
    for bad in [-0.1, f64::NAN, f64::INFINITY] {
        let err = normalize_weights(&[0.5, bad]).expect_err("bad weight");
        assert_eq!(err.code, ErrorCode::OutOfRange, "for {bad}");
    }
}

/// One weight is the whole direction, so it normalizes to 1.0.
#[test]
fn normalize_weights_normalizes_a_single_weight_to_one() {
    let out = normalize_weights(&[2.0]).expect("positive sum");
    assert_eq!(out.len(), 1, "one weight in, one out");
    assert!((out[0] - 1.0).abs() < 1e-12, "got {out:?}");
}

/// No weights have no direction to rescale, so an empty slice is rejected
/// before the sum is used as a divisor.
#[test]
fn normalize_weights_rejects_an_empty_slice() {
    let err = normalize_weights(&[]).expect_err("nothing to rescale");
    assert_eq!(err.code, ErrorCode::OutOfRange);
}
