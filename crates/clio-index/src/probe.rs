// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the setup-time health check of a configured
//! service endpoint, and for nothing else.
//!
//! ## Owns
//! - `endpoint`, which reports whether a URL answers and carries an API key
//!
//! ## Does not own
//! - The transport, its TLS setup, its timeout, and its error mapping (`http`)
//! - The rule that refuses a remote plain-`http` endpoint (`http::gate`)
//! - What a healthy endpoint means for the run (the caller decides)
//!
//! ## Boundary
//!
//! Changes to the setup probe should normally require changes here. Changes to
//! the transport should not require changes here — this module has no agent of
//! its own, which is the point.
//!
//! # Why there is no second agent
//!
//! A probe that built its own HTTP client could disagree with the one the real
//! request path uses: different roots, different TLS settings, a different
//! answer to "is this service healthy". Setup would then report a service fine
//! while the request that matters would fail. So the probe calls the same
//! functions, over the same transport, and reports what they say.

use crate::http::get_ok;
use clio_types::AmError;
use std::time::Duration;

/// How long the setup check waits for one answer.
///
/// Setup is a person watching a screen, so this is short: long enough for a
/// sidecar that is already running, short enough that a wrong address is
/// reported rather than waited out.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Ask `url` whether it answers, sending `api_key` if one was given.
///
/// # Errors
/// The endpoint is refused by the request gate, or the transport failed. Both
/// arrive as the transport's own error, so the operator reads what actually went
/// wrong rather than a summary written here.
pub fn endpoint(url: &str, api_key: Option<&str>) -> Result<String, AmError> {
    get_ok(url, PROBE_TIMEOUT, api_key)?;
    Ok("answered".to_owned())
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod probe_tests;
