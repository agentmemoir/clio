// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for validating config set paths, values, and mutation scopes.
//!
//! ## Owns
//! - Versioned allowlist of settable paths and typed value checks
//! - Forbidden prefixes (`gates.*`, `ranking.*`, `fr10_satisfied`, …)
//! - `ConfigScope` parsing
//!
//! ## Does not own
//! - Writing overlays into `Runtime` state
//! - Secret masking of validated values
//! - Numeric range rules for the paths that carry a number (see `validate_set`)
//!
//! ## Boundary
//!
//! Changes to what may be set via `config_set` should normally require changes here.
//! Changes to merge order or tool envelopes should not require changes here.
//!
//! Keep this module focused on config path/value and scope validation.

use super::validate_set::validate_numeric_value;
use crate::error::{AmError, ErrorCode};
use crate::log::LogLevel;
use clio_types::{is_loopback_host, parse_scheme_host};
use serde_json::Value;

/// Config schema version stamped into effective defaults.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Highest configurable embedding width (the storage engine's ceiling).
pub const MAX_EMBED_DIMS: u64 = 16_000;

/// Paths callers may set via `config_set` (schema v1).
const ALLOWED_SET_PATHS: &[&str] = &[
    "log.level",
    "injection.budget_tokens",
    "history.lesson_max_tokens",
    "history.similarity_threshold",
    "history.default_limit",
    "history.max_limit",
    "backend",
    "bank.fixed",
    "bank.policy",
    "credentials.api_key",
    "sync.omit",
    "store.database_url",
    "store.sqlite_path",
    "extract.provider",
    "extract.url",
    "extract.model",
    "extract.temperature",
    "extract.day_first",
    "embed.url",
    "embed.model",
    "embed.dims",
    "embed.provider",
    "embed.switch_space",
    "rerank.url",
    "rerank.model",
    "rerank.provider",
    "rerank.max_batch",
    "credentials.embed_api_key",
    "credentials.rerank_api_key",
    "credentials.extract_api_key",
];

/// Prefixes that must never be set (gate control, ranking knobs, packaging claims).
const FORBIDDEN_PREFIXES: &[&str] = &[
    "gates.",
    "ranking.",
    "fr10_satisfied",
    "mcp.",
    "profile.",
    "retention.",
    "schema_version",
];

/// Config mutation scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    Session,
    Profile,
    Deployment,
}

impl ConfigScope {
    /// Parse scope string from tool args.
    ///
    /// # Errors
    /// Unknown scope names.
    pub fn parse(raw: &str) -> Result<Self, AmError> {
        match raw {
            "session" => Ok(Self::Session),
            "profile" => Ok(Self::Profile),
            "deployment" => Ok(Self::Deployment),
            other => Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("scope must be session|profile|deployment, got `{other}`"),
            )),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Profile => "profile",
            Self::Deployment => "deployment",
        }
    }
}

/// Validate that `path` is allowlisted and not a forbidden gate/ranking/packaging key.
///
/// # Errors
/// Forbidden, unknown, or empty paths.
pub(crate) fn validate_set_path(path: &str) -> Result<(), AmError> {
    if path.is_empty() || path.contains("..") {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "config path must be a non-empty dotted path",
        ));
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("bypass")
        || FORBIDDEN_PREFIXES
            .iter()
            .any(|p| lower == p.trim_end_matches('.') || lower.starts_with(p))
        || lower == "gates"
        || lower.starts_with("gates.")
    {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            format!(
                "path `{path}` is forbidden (gates/ranking/packaging are not tunable via config_set; use ranking_env_set for ranking knobs)"
            ),
        ));
    }
    if !ALLOWED_SET_PATHS.contains(&path) {
        return Err(AmError::new(
            ErrorCode::UnknownPath,
            format!("path `{path}` is not in config schema v{CONFIG_SCHEMA_VERSION} allowlist"),
        ));
    }
    Ok(())
}

/// Type/range checks for allowlisted paths.
///
/// The numeric paths are answered by [`validate_numeric_value`], which owns
/// their ranges and their rejection messages; this match remains the single
/// entry point every caller uses.
///
/// # Errors
/// Invalid types or out-of-range values.
#[expect(clippy::too_many_lines)] // path matrix is clearer as one match.
pub(crate) fn validate_set_value(path: &str, value: &Value) -> Result<(), AmError> {
    match path {
        "injection.budget_tokens"
        | "history.lesson_max_tokens"
        | "history.default_limit"
        | "history.max_limit"
        | "history.similarity_threshold"
        | "extract.temperature"
        | "embed.dims"
        | "rerank.max_batch" => validate_numeric_value(path, value)?,
        "log.level" => {
            let Some(s) = value.as_str() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "log.level must be a string",
                ));
            };
            if LogLevel::parse(s).is_none() {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("log.level `{s}` is not error|warn|info|debug|trace"),
                ));
            }
        }
        "backend" => {
            let Some(s) = value.as_str() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "backend must be a string",
                ));
            };
            if s != "sqlite" && s != "postgres" {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    "backend must be sqlite|postgres (hint only; behavior unchanged)",
                ));
            }
        }
        "bank.fixed" => {
            let Some(s) = value.as_str() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "bank.fixed must be a string",
                ));
            };
            if s.is_empty() {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    "bank.fixed must be non-empty",
                ));
            }
        }
        "bank.policy" => {
            serde_json::from_value::<crate::context::BankDefaultPolicy>(value.clone()).map_err(
                |e| {
                    AmError::new(
                        ErrorCode::InvalidType,
                        format!("bank.policy type error: {e}"),
                    )
                },
            )?;
        }
        "credentials.api_key"
        | "credentials.embed_api_key"
        | "credentials.rerank_api_key"
        | "credentials.extract_api_key"
        | "embed.model"
        | "rerank.model"
        | "extract.model" => {
            if !value.is_string() {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    format!("{path} must be a string"),
                ));
            }
        }
        "embed.url" | "rerank.url" | "extract.url" => {
            let Some(s) = value.as_str() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    format!("{path} must be a string"),
                ));
            };
            validate_endpoint_value(path, s)?;
        }
        "sync.omit" => match value {
            Value::Bool(_) => {}
            _ => {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "sync.omit must be a boolean",
                ));
            }
        },
        "store.database_url" => {
            if !value.is_string() {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "store.database_url must be a string",
                ));
            }
        }
        "store.sqlite_path" => {
            let Some(s) = value.as_str() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "store.sqlite_path must be a string",
                ));
            };
            if s.is_empty() {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    "store.sqlite_path must be non-empty",
                ));
            }
        }
        "extract.day_first" => match value {
            Value::Bool(_) | Value::Null => {}
            _ => {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "extract.day_first must be a boolean or null",
                ));
            }
        },
        "extract.provider" => {
            let Some(s) = value.as_str() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "extract.provider must be a string",
                ));
            };
            if !s.is_empty() && s != "template" && s != "openai" {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!(
                        "extract.provider must be template|openai (or empty = template), got `{s}`"
                    ),
                ));
            }
        }
        "embed.provider" | "rerank.provider" => {
            let Some(s) = value.as_str() else {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    format!("{path} must be a string"),
                ));
            };
            let allowed: &[&str] = if path == "embed.provider" {
                &["tei", "openai"]
            } else {
                &["tei", "cohere"]
            };
            // An explicitly empty provider is rejected rather than silently
            // meaning "unset"; an omitted field simply never reaches this check
            // and receives the tei default at read time.
            if !allowed.contains(&s) {
                return Err(AmError::new(
                    ErrorCode::OutOfRange,
                    format!("{path} must be {}, got `{s}`", allowed.join("|")),
                ));
            }
        }
        "embed.switch_space" => match value {
            Value::Bool(_) => {}
            _ => {
                return Err(AmError::new(
                    ErrorCode::InvalidType,
                    "embed.switch_space must be a boolean",
                ));
            }
        },
        _ => {}
    }
    Ok(())
}

/// Refuse a remote plain-http service endpoint.
///
/// The reason is concrete, not stylistic. A hosted embedding or reranking
/// service is sent the API key the operator just typed, as a request header, on
/// every call, together with every piece of text being embedded. Over plain
/// `http` all of it crosses the network in the clear, and a configuration
/// screen that allows it without a word is worse than one that cannot express
/// it.
///
/// Three things this rule deliberately does not do:
///
/// - It does not touch loopback. The local sidecars are on this machine, which
///   is the whole reason they can stay on `http`.
/// - It does not rewrite `http` to `https`. Quietly changing an address the
///   operator typed can send traffic to a port nothing is listening on, and the
///   failure then looks like a service problem rather than a mistake.
/// - It does not judge the rest of the value. A value this cannot read as a URL
///   is left to the request gate, which parses URLs the way the transport does
///   and refuses at the moment a request would be sent.
///
/// # Errors
/// The endpoint is plain `http` to a host that is not this machine.
fn validate_endpoint_value(path: &str, value: &str) -> Result<(), AmError> {
    let trimmed = value.trim();
    let Ok((scheme, host)) = parse_scheme_host(trimmed) else {
        return Ok(());
    };
    if scheme != "http" || is_loopback_host(&host) {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::InvalidArgument,
        format!(
            "{path} must use https for a remote endpoint: `{trimmed}` is plain http, so the API key \
             and every text sent to it would cross the network unencrypted. Change the scheme to \
             `https://{host}`, or point it at a service on this machine with a loopback address \
             such as `http://127.0.0.1:<port>`"
        ),
    ))
}

#[cfg(test)]
#[path = "validate_endpoint_tests.rs"]
mod endpoint_tests;
#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
