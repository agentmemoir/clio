// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the record-shaped batch
//! pre-flight helpers: admission-decision rejection mapping.
//!
//! ## Owns
//! - Rejection-detail assertions for record-shaped batch pre-flight
//!
//! ## Does not own
//! - Batch op decoding or `$ref` resolution (see `batch_preflight.rs`)
//! - Transactional commit (see `clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::rejection;
use clio_admission::decision::Decision;

#[test]
fn rejection_helper_maps_decision_reason() {
    let decision = Decision {
        pass: false,
        admission_score: 0.2,
        factors: clio_admission::decision::FactorBreakdown {
            utility: 0.1,
            confidence: 0.1,
            novelty: 0.1,
            recency: 0.1,
            type_prior: 0.1,
        },
        rejection_reason: Some("below theta".into()),
        profile_version: 1,
        mission_rule_ids: Vec::new(),
        mission_rule_labels: Vec::new(),
    };
    let err = rejection(&decision);
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
    assert_eq!(err.message, "below theta");

    // A decision with no explicit reason falls back to the default message.
    let blank = rejection(&Decision {
        rejection_reason: None,
        ..decision
    });
    assert_eq!(blank.message, "admission rejected");
}
