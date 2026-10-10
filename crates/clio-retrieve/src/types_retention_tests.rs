// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the retention knobs on [`RetrieveRequest`]: the defaults
//! [`RetrieveRequest::new`] must set so a caller that does not mention the
//! curve keeps the order it had before the knobs existed.
//!
//! ## Owns
//! - Constructor-default assertions for `retention_curve` and `retention_floor`
//!
//! ## Does not own
//! - The curve arithmetic (`clio_types::decay`) or its application to scores
//!   (`hybrid_rank`)
//! - The compose-request defaults (`compose_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the request defaults change; they never shape a score.
//!
//! Keep this module focused on the retrieve-request retention defaults.

use super::*;

#[test]
fn new_request_defaults_keep_the_curve_off() {
    let req = RetrieveRequest::new("bank", "query");
    assert!(!req.retention_curve, "curve is off by default");
    assert_eq!(req.retention_floor, 0.25, "floor default is 0.25");
}
