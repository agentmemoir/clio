// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the admission knobs: the
//! knob defaults, the known tuning-key list, and knob validation through
//! the ranking patch surface.
//!
//! ## Owns
//! - Tests for the admission knob defaults and the known tuning-key list
//! - Tests for per-key override acceptance and rejection through patches
//! - Tests for the coactivation constant bounds and their validation
//!
//! ## Does not own
//! - The knob tables and the validation logic itself (see `admission_knobs`)
//! - The patch merge that applies validated knobs (see `ranking`)
//!
//! ## Boundary
//!
//! Changes to admission knob validation or its defaults should normally
//! require a change here. A change to the ranking merge logic should not.
//!
//! Keep this module focused on the admission knob tests.

use super::*;
use crate::admission_knobs::{CoactivationConstants, default_recency_tau, validate_coactivation};
use crate::ranking::{RankingEnv, RankingEnvPatch, preview_patch};
use clio_types::SemanticCategory;
use std::collections::HashMap;

#[test]
fn knob_defaults_follow_appendix() {
    assert!((DEFAULT_THETA_ADMIT - 0.55).abs() < 1e-9);
    assert!((DEFAULT_RECENCY_TAU_DAYS - 30.0).abs() < 1e-9);
    assert!((default_type_prior("persona").unwrap() - 0.90).abs() < 1e-9);
    assert!((default_type_prior("gist").unwrap() - 0.40).abs() < 1e-9);
    assert!(default_type_prior("nope").is_none());
    assert!((prior_for(&HashMap::new(), "unknown-key") - 0.5).abs() < 1e-9);
    assert!((default_recency_tau() - 30.0).abs() < 1e-9);
    assert_eq!(KNOWN_ADMISSION_KEYS.len(), 11);
}

#[test]
fn admission_keys_start_with_the_single_source_semantic_categories() {
    // The tuning-key list opens with the categories; tie that prefix to the
    // one whitelist so a new category cannot be silently missed here.
    let categories = SemanticCategory::WIRE;
    assert_eq!(&KNOWN_ADMISSION_KEYS[..categories.len()], categories);
}

#[test]
fn belief_create_key_is_tunable() {
    // The belief-writes key is accepted by knob validation and resolves
    // like any other key: override wins, documented default prior otherwise.
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            theta_overrides: Some(HashMap::from([("belief".into(), 0.7)])),
            type_prior_overrides: Some(HashMap::from([("belief".into(), 0.6)])),
            ..RankingEnvPatch::default()
        },
    )
    .expect("belief overrides are known keys");
    assert!((threshold_for(&next.theta_overrides, next.theta_admit, "belief") - 0.7).abs() < 1e-9);
    assert!((prior_for(&next.type_prior_overrides, "belief") - 0.6).abs() < 1e-9);
    // No override: prior falls back to the documented 0.5 default, theta to global.
    assert!((default_type_prior("belief").unwrap() - 0.5).abs() < 1e-9);
    assert!((prior_for(&HashMap::new(), "belief") - 0.5).abs() < 1e-9);
    assert!((threshold_for(&HashMap::new(), 0.55, "belief") - 0.55).abs() < 1e-9);
}

#[test]
fn per_key_overrides_apply() {
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            theta_overrides: Some(HashMap::from([("persona".into(), 0.9)])),
            type_prior_overrides: Some(HashMap::from([("gist".into(), 0.6)])),
            recency_tau_days: Some(7.0),
            ..RankingEnvPatch::default()
        },
    )
    .expect("overrides");
    assert!((threshold_for(&next.theta_overrides, next.theta_admit, "persona") - 0.9).abs() < 1e-9);
    assert!((threshold_for(&next.theta_overrides, next.theta_admit, "schema") - 0.55).abs() < 1e-9);
    assert!((prior_for(&next.type_prior_overrides, "gist") - 0.6).abs() < 1e-9);
}

#[test]
fn rejects_bad_override_keys_values_and_bypass() {
    let base = RankingEnv::system_defaults();
    for patch in [
        RankingEnvPatch {
            theta_overrides: Some(HashMap::from([("nope".into(), 0.5)])),
            ..RankingEnvPatch::default()
        },
        RankingEnvPatch {
            theta_overrides: Some(HashMap::from([("persona".into(), 2.0)])),
            ..RankingEnvPatch::default()
        },
        RankingEnvPatch {
            type_prior_overrides: Some(HashMap::from([("admission_bypass".into(), 0.0)])),
            ..RankingEnvPatch::default()
        },
        RankingEnvPatch {
            recency_tau_days: Some(0.0),
            ..RankingEnvPatch::default()
        },
    ] {
        assert!(preview_patch(&base, &patch).is_err());
    }
    assert!(validate_coactivation(&CoactivationConstants::default()).is_ok());
}

#[test]
fn multi_error_overrides_report_sorted_first_key() {
    let base = RankingEnv::system_defaults();
    let err = preview_patch(
        &base,
        &RankingEnvPatch {
            theta_overrides: Some(HashMap::from([
                ("zzz_bad".into(), 0.5),
                ("aaa_bad".into(), 0.5),
            ])),
            ..RankingEnvPatch::default()
        },
    )
    .expect_err("two bad keys");
    assert!(err.message.contains("aaa_bad"), "got: {err}");
}

#[test]
fn patch_without_new_knobs_fills_their_defaults() {
    // The env is no longer deserialized as a whole, so the omitted-field
    // defaults are exercised through the patch surface.
    let patch: RankingEnvPatch = serde_json::from_value(serde_json::json!({
        "coactivation": {"eta": 0.15, "half_life_days": 30.0, "w_min": 0.05}
    }))
    .expect("coactivation group with omitted bounds is accepted");
    let env = preview_patch(&RankingEnv::system_defaults(), &patch).expect("patch applies");
    assert!((env.recency_tau_days - 30.0).abs() < 1e-9);
    assert!(env.theta_overrides.is_empty());
    assert!(env.type_prior_overrides.is_empty());
    // New association bounds fall back to their documented defaults.
    assert_eq!(env.coactivation.hub_degree_limit, 16);
    assert_eq!(env.coactivation.max_hops, 2);
}

#[test]
fn ranking_env_patch_updates_association_knobs() {
    let base = RankingEnv::system_defaults();
    let next = preview_patch(
        &base,
        &RankingEnvPatch {
            coactivation: Some(CoactivationConstants {
                eta: 0.5,
                half_life_days: 10.0,
                w_min: 0.2,
                hub_degree_limit: 3,
                max_hops: 4,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("assoc knobs patch");
    assert!((next.coactivation.eta - 0.5).abs() < 1e-12);
    assert!((next.coactivation.w_min - 0.2).abs() < 1e-12);
    assert_eq!(next.coactivation.hub_degree_limit, 3);
    assert_eq!(next.coactivation.max_hops, 4);
    // Round-trips through serde so `ranking_env_get` exposes the new knobs.
    let json = serde_json::to_value(&next).expect("serialize");
    assert_eq!(
        json["coactivation"]["hub_degree_limit"],
        serde_json::json!(3)
    );
    assert_eq!(json["coactivation"]["max_hops"], serde_json::json!(4));
}

#[test]
fn hub_and_expand_bounds_validate() {
    let defaults = CoactivationConstants::default();
    assert_eq!(defaults.hub_degree_limit, 16);
    assert_eq!(defaults.max_hops, 2);
    assert!(validate_coactivation(&defaults).is_ok());
    let mut bad_hub = defaults.clone();
    bad_hub.hub_degree_limit = 0;
    assert!(validate_coactivation(&bad_hub).is_err());
    let mut bad_hops = defaults;
    bad_hops.max_hops = 0;
    assert!(validate_coactivation(&bad_hops).is_err());
}

#[test]
fn ranking_env_set_rejects_bypass_patch_shape() {
    let mut rt = crate::config::Runtime::with_env(&|_| None);
    let out = rt.call_tool(
        "ranking_env_set",
        &serde_json::json!({"patch": {"admission_bypass": true}}),
    );
    assert_eq!(out["ok"], serde_json::json!(false));
}
