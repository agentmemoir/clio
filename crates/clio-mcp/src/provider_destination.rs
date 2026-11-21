// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deciding whether a caller-supplied provider
//! base URL names a destination the server may send an authenticated request to.
//!
//! ## Owns
//! - The scheme gate (`http`/`https`) that runs before any other rule
//! - Host normalisation (case, one trailing dot) and the operator allowlist match
//! - The address-class refusal for a host the allowlist does not name
//! - Reading the allowlist out of the effective config document
//!
//! ## Does not own
//! - Address classification itself (`clio_types::net::is_denied_destination`)
//! - The URL parse (`clio_types::net::http_target`)
//! - Pagination, item mapping, or the request (`provider_adapter`)
//! - Where the operator value is stored (`clio-config`)
//!
//! ## Boundary
//!
//! Changes to which destination a provider pull may reach should normally
//! require changes here. Changes to what counts as a denied address should not
//! require changes here, and changes to the pull itself should not either.
//!
//! Keep this module focused on destination trust for the provider adapter.

use std::net::IpAddr;

use clio_types::net::http_target;
use clio_types::{AmError, BUILT_IN_PROVIDER_HOST, ErrorCode, HttpTarget, is_denied_destination};
use serde_json::Value;

use crate::runtime::McpState;

/// The config path the allowlist is stored at, as the effective document spells it.
const ALLOW_HOSTS_POINTER: &str = "/provider/allow_hosts";

/// Accept `url` as a provider destination, or refuse it naming the host and the
/// rule that refused it.
///
/// The checks run in one order and fail closed. The scheme is read first,
/// because no address rule can make an unsupported transport safe. The host is
/// then normalised and matched against `allow_hosts`: an entry there is
/// authoritative, so an operator who deliberately points the tool at an internal
/// provider keeps it working. Only a host the allowlist does not name is judged
/// by its address class, and a host that is neither allowlisted nor a denied
/// address is still refused, because reaching an arbitrary public host on a
/// caller's word is the same defect one hop further out.
///
/// # Errors
/// The scheme is not `http`/`https`, the host is a denied address, or the host
/// is not in the operator allowlist. Every one is [`ErrorCode::Forbidden`].
pub(crate) fn check_provider_target(
    url: &str,
    allow_hosts: &[&str],
) -> Result<HttpTarget, AmError> {
    let target = http_target(url).map_err(|reason| {
        AmError::new(
            ErrorCode::Forbidden,
            format!("provider base URL refused: {reason}"),
        )
    })?;
    let host = normalise_host(&target.host);
    if allow_hosts
        .iter()
        .any(|allowed| allowed.trim().eq_ignore_ascii_case(&host))
    {
        return Ok(HttpTarget {
            scheme: target.scheme,
            host,
        });
    }
    // A host that is not an IP literal fails this parse, so a name reaches the
    // allowlist refusal below instead of being resolved here.
    if let Ok(ip) = host.parse::<IpAddr>()
        && is_denied_destination(ip)
    {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            format!(
                "provider base URL refused: host `{host}` is a loopback, private, link-local, \
                 metadata, carrier-grade NAT or otherwise non-public address, and the operator \
                 allowlist does not name it"
            ),
        ));
    }
    Err(AmError::new(
        ErrorCode::Forbidden,
        format!(
            "provider base URL refused: host `{host}` is not in the operator allowlist \
             (config path `provider.allow_hosts`, environment `CLIO_PROVIDER_ALLOW_HOSTS`); \
             add it there to reach this provider"
        ),
    ))
}

/// The operator allowlist for this deployment.
///
/// Read from the effective config document on every call, at the same
/// `/provider/allow_hosts` pointer the `config_get` view reports, so the tool and
/// the operator cannot disagree about it. An absent pointer, a value that is not
/// a list of strings, and a poisoned config lock all fall back to the built-in
/// provider host: the shipped provider then keeps working with no setup, and a
/// misread can only narrow the list, never widen it.
pub(crate) fn allow_hosts_from(state: &McpState) -> Vec<String> {
    let Ok(runtime) = state.config.lock() else {
        return vec![BUILT_IN_PROVIDER_HOST.to_owned()];
    };
    let effective = runtime.effective_raw();
    let hosts: Vec<String> = effective
        .pointer(ALLOW_HOSTS_POINTER)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|host| !host.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if hosts.is_empty() {
        return vec![BUILT_IN_PROVIDER_HOST.to_owned()];
    }
    hosts
}

/// Refuse `url` as a provider destination under this deployment's allowlist.
///
/// # Errors
/// As [`check_provider_target`].
pub(crate) fn require_provider_target(state: &McpState, url: &str) -> Result<(), AmError> {
    let hosts = allow_hosts_from(state);
    let refs: Vec<&str> = hosts.iter().map(String::as_str).collect();
    check_provider_target(url, &refs).map(|_| ())
}

/// The provider base URL the caller asked for, or `default`, once it has passed
/// the destination rule.
///
/// Resolving and judging in one place is what keeps the judgement from being
/// skipped: there is one door into a provider request, and the address it opens
/// is the address that was judged.
///
/// # Errors
/// As [`check_provider_target`].
pub(crate) fn checked_base_url<'a>(
    state: &McpState,
    options: &'a Value,
    default: &'a str,
) -> Result<&'a str, AmError> {
    let url = options
        .get("api_base_url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            options
                .get("base_url")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(default);
    require_provider_target(state, url)?;
    Ok(url)
}

/// The host as a comparison key: lowercased, with exactly one trailing dot
/// removed.
///
/// An absolute DNS name may carry a trailing dot (`api.mem0.ai.`), and the split
/// keeps it because the dot is part of the name as typed. It names the same host,
/// so leaving it on would let a spelling of an allowlisted host slip past the
/// allowlist and land in the address rules; removing one dot is the most that can
/// be removed without reacting to a malformed name.
fn normalise_host(host: &str) -> String {
    let lowered = host.trim().to_ascii_lowercase();
    lowered.strip_suffix('.').unwrap_or(&lowered).to_owned()
}

#[cfg(test)]
#[path = "provider_destination_tests.rs"]
mod provider_destination_tests;
