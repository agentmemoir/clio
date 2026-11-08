// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one release-metadata request the version
//! check makes: the endpoint it reads, its timeouts and response cap, and the
//! tag the answer names.
//!
//! ## Owns
//! - The release endpoint and its three bounds: connect, total, and body size
//! - The single request, sent through the workspace's one approved client
//! - Reading the body under the cap, and the `tag_name` a 200 answer states
//!
//! ## Does not own
//! - Comparing the tag with an installed version (`version_check`)
//! - The version text's grammar and ordering (`version_check_semver`)
//! - Any installation evidence, and any output (`version_check`)
//!
//! ## Boundary
//!
//! Changes to which address is read, or to the bounds a request has, should
//! normally require changes here. Changes to what the command prints should not.
//!
//! Only release metadata crosses the network. No asset is downloaded, no file is
//! written, and no update is applied: the answer's tag is read and nothing else
//! is touched.
//!
//! Every failure is `None` rather than an error type, because the command reports
//! all of them the same way: an unavailable release. A redirect is not followed,
//! so at most one request leaves the process.
//!
//! Keep this module focused on the release-metadata request.

use crate::version_check_semver::Version;
use std::io::Read;
use std::time::Duration;

/// The one release endpoint `clio version --check` reads.
pub(crate) const RELEASE_ENDPOINT: &str =
    "https://api.github.com/repos/agentmemoir/clio/releases/latest";

/// How long the connection may take to open.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the whole exchange may take, connection included.
pub(crate) const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

/// The most bytes of an answer that are read. A larger answer is refused rather
/// than buffered, so a wrong or hostile address cannot decide this process's
/// memory use.
pub(crate) const MAX_RESPONSE_BYTES: usize = 1_000_000;

/// The published version at `endpoint`, or `None` when no valid release can be
/// established from it.
///
/// `None` covers every reason: the connection could not be opened, the exchange
/// ran past its deadline, the answer was any status but 200 (a rate limit, a
/// redirect, or a refusal), the body passed the cap, the body was not JSON, it
/// stated no `tag_name`, or the tag is not a version this program compares.
///
/// The endpoint is an argument rather than the constant so a test can point the
/// one request at a local listener and prove what it sends and what it accepts.
pub(crate) fn latest_release(endpoint: &str) -> Option<Version> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // The status is a fact this module reads, not a transport failure.
        .http_status_as_error(false)
        // No redirect is followed: one request leaves the process, whatever the
        // answer says.
        .max_redirects(0)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(TOTAL_TIMEOUT))
        .build()
        .into();
    let response = agent
        .get(endpoint)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "clio")
        .call()
        .ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let mut body = response.into_body();
    let bytes = read_capped(&mut body)?;
    let metadata: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Version::parse(metadata.get("tag_name")?.as_str()?)
}

/// The whole body, or `None` when it reads past the cap or the read fails. The
/// cap is checked as the bytes arrive, so an answer that never ends is refused
/// at the cap rather than after it.
fn read_capped(body: &mut ureq::Body) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    let mut reader = body.as_reader();
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Some(out),
            Ok(read) => {
                out.extend_from_slice(&chunk[..read]);
                if out.len() > MAX_RESPONSE_BYTES {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
}
