// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Policy resolution tests.

use super::*;
use clio_config::ranking::{RankingEnv, RankingEnvPatch, preview_patch};
use std::collections::HashMap;

#[test]
fn defaults_are_the_shipped_values() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    assert!((policy.threshold_for("persona") - 0.4375).abs() < 1e-9);
    assert!((policy.threshold_for("gist") - 0.4375).abs() < 1e-9);
    assert!((policy.prior_for("persona") - 0.90).abs() < 1e-9);
    assert!((policy.prior_for("gist") - 0.40).abs() < 1e-9);
    for w in policy.weights() {
        assert!((w - 0.25).abs() < 1e-9);
    }
}

#[test]
fn overrides_win_over_defaults() {
    let env = preview_patch(
        &RankingEnv::system_defaults(),
        &RankingEnvPatch {
            theta_overrides: Some(HashMap::from([("persona".into(), 0.95)])),
            type_prior_overrides: Some(HashMap::from([("gist".into(), 0.6)])),
            ..RankingEnvPatch::default()
        },
    )
    .expect("patch");
    let policy = AdmissionPolicy::new(&env);
    assert!((policy.threshold_for("persona") - 0.95).abs() < 1e-9);
    assert!((policy.threshold_for("schema") - 0.4375).abs() < 1e-9);
    assert!((policy.prior_for("gist") - 0.6).abs() < 1e-9);
}
