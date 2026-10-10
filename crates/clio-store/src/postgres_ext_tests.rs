// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres extension-probe tests.
//!
//! ## Owns
//! - `require_extension` pass and fail paths against a live Postgres
//!
//! ## Does not own
//! - Store migrations or MemTree persistence tests
//!
//! ## Boundary
//!
//! Changes to extension-probe scenarios should normally require changes here.
//! Changes to extension handling itself should not require changes here.
//!
//! Keep this module focused on Postgres extension probe coverage.

use super::require_extension;
use clio_types::ErrorCode;

#[test]
fn pgvector_probe_passes_present_and_fails_missing() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let mut client = crate::pg_tls::connect(url.as_str()).unwrap();
    require_extension(&mut client, "vector").unwrap();
    let err = require_extension(&mut client, "no_such_ext_xyz").expect_err("missing ext");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("not installed on the server"));
}
