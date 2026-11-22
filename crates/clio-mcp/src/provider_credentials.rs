// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for deciding which credential a provider pull may
//! offer to a destination, and for the redacted view of that decision.
//!
//! ## Owns
//! - The caller-supplied credential forms (`credentials` as a string or object)
//! - The server key fallback (`CLIO_MEM0_API_KEY`) and the allowlist gate on it
//! - The refusal of a credential-less pull at a destination the operator did not
//!   sanction
//! - The fully-redacted report of accepted credentials
//!
//! ## Does not own
//! - Whether the destination may be reached (`provider_destination`)
//! - Pagination, item mapping, or the request (`provider_adapter`)
//! - Storing or sealing secrets (`clio-config`)
//!
//! ## Boundary
//!
//! Changes to which credential wins, or to what a report may claim was
//! accepted, should normally require changes here. Changes to the destination
//! rule should not require changes here.
//!
//! Keep this module focused on credential selection for provider pulls.

use clio_config::secret::{mask_secret, masked_clone};
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};

/// The environment variable holding the server's own provider key.
pub(crate) const SERVER_KEY_VAR: &str = "CLIO_MEM0_API_KEY";

/// The object keys an `import_provider` credential may be read from.
///
/// One list feeds both the extractor and the report, so a key that can reach the
/// `Authorization` header is always a key the report masks.
pub(crate) const ACCEPTED_CREDENTIAL_KEYS: &[&str] = &["api_key", "token", "key", "secret"];

/// The `Authorization` header value for a provider pull, or the refusal that
/// stops a pull from being sent to a destination the operator never sanctioned.
///
/// `sanctioned` says the resolved destination is one the operator allowlist
/// names, and it decides two things. The server key is offered only to a
/// sanctioned host, because pairing an operator secret with a host the caller
/// chose would send it where the operator never pointed it. And an unsanctioned
/// destination is refused here whether or not the caller supplied a credential,
/// so the refusal cannot be escaped by omitting one.
///
/// `Ok(None)` is a pull with no credential at a sanctioned destination, which is
/// how a provider that needs no key stays reachable.
///
/// # Errors
/// The destination is not allowlisted: [`ErrorCode::Forbidden`], the code the
/// destination rule itself refuses with.
pub(crate) fn authorization(
    args: &Value,
    env: &dyn Fn(&str) -> Option<String>,
    sanctioned: bool,
) -> Result<Option<String>, AmError> {
    if !sanctioned {
        return Err(AmError::new(ErrorCode::Forbidden, refusal_message()));
    }
    Ok(extract_api_key(args, env, sanctioned).map(|key| header_value(&key)))
}

/// The caller's credential, or the server's own key when `sanctioned`.
pub(crate) fn extract_api_key(
    args: &Value,
    env: &dyn Fn(&str) -> Option<String>,
    sanctioned: bool,
) -> Option<String> {
    caller_credential(args).or_else(|| {
        sanctioned
            .then(|| env(SERVER_KEY_VAR))
            .flatten()
            .filter(|s| !s.trim().is_empty())
    })
}

/// Fully-redacted view of accepted credentials (never plaintext).
///
/// Gated on `sanctioned` for the same reason [`extract_api_key`] is: a report
/// naming the server key at a destination it is never sent to would describe a
/// credential the caller does not have.
pub(crate) fn masked_credentials(
    args: &Value,
    env: &dyn Fn(&str) -> Option<String>,
    sanctioned: bool,
) -> Value {
    if let Some(creds) = args.get("credentials") {
        return masked_credential_value(creds);
    }
    if !sanctioned {
        return Value::Null;
    }
    env(SERVER_KEY_VAR).map_or(Value::Null, |key| json!({ "api_key": mask_secret(&key) }))
}

fn refusal_message() -> String {
    format!(
        "provider destination refused: the server key in `{SERVER_KEY_VAR}` is offered only to a \
         host the operator allowlist names, so this destination cannot be reached"
    )
}

fn caller_credential(args: &Value) -> Option<String> {
    let creds = args.get("credentials")?;
    if let Some(s) = creds.as_str()
        && !s.trim().is_empty()
    {
        return Some(s.trim().to_owned());
    }
    let obj = creds.as_object()?;
    ACCEPTED_CREDENTIAL_KEYS
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|val| !val.is_empty())
        .map(str::to_owned)
}

/// The redacted report of a caller-supplied `credentials` value.
///
/// Every shape the extractor can read is masked here: a bare string is the
/// credential itself, an object is masked under [`ACCEPTED_CREDENTIAL_KEYS`] so
/// the reported keys and the accepted keys cannot drift, and an array is masked
/// element by element. Anything else is an operand-free shape the extractor
/// ignores, so it is passed through the general masker.
#[must_use]
pub(crate) fn masked_credential_value(creds: &Value) -> Value {
    match creds {
        Value::String(s) => Value::String(mask_secret(s)),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), masked_member(key, value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(masked_credential_value).collect()),
        other => masked_clone(other),
    }
}

/// One member of a credential object: accepted keys are masked whatever their
/// type; every other key keeps the general masking rule.
fn masked_member(key: &str, value: &Value) -> Value {
    if ACCEPTED_CREDENTIAL_KEYS.contains(&key) {
        return Value::String(mask_secret(""));
    }
    masked_clone(value)
}

fn header_value(key: &str) -> String {
    if key.starts_with("Token ") || key.starts_with("Bearer ") {
        key.to_owned()
    } else {
        format!("Token {key}")
    }
}

#[cfg(test)]
#[path = "provider_credentials_tests.rs"]
mod provider_credentials_tests;
