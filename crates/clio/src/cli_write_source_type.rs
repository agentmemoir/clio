// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `--epistemic-kind`/`--source-type` flag
//! pair on the candidate write verbs: `--source-type` is required with belief,
//! refused with fact, and its value must be one of the published wire values.
//!
//! ## Owns
//! - The valid `--source-type` values, taken from `SourceType` so they cannot drift
//! - The pairing check that turns a mismatch into a usage error naming both flags
//!
//! ## Does not own
//! - The `remember`/`admit` flag decoding that calls it (see `cli_write_core`)
//! - The domain rule itself (see `clio-types`)
//!
//! ## Boundary
//!
//! Changes to the CLI spelling of the pairing should normally require changes
//! here. Changes to the domain `source_type` rule should not.
//!
//! Keep this module focused on the one flag pair.

use crate::cli_args::ParsedArgs;
use crate::cli_error::CliError;
use crate::cli_read_args::validate_choice;
use clio_types::SourceType;

/// The valid `--source-type` values, from the enum so the list cannot drift.
const SOURCE_TYPES: &[&str] = &[
    SourceType::UserStated.as_str(),
    SourceType::AgentInferred.as_str(),
    SourceType::ThirdParty.as_str(),
];

/// Resolve `--source-type` against `--epistemic-kind`.
///
/// The two flags are a pair: a belief needs a source type and a fact must not
/// carry one. Checked here, at the flag boundary with every other caller-supplied
/// flag value, so a mistake is a usage error that names both flags and the valid
/// values rather than an `invalid_argument` from the domain layer. Returns the
/// value to forward, or `None` when the flag was absent.
pub(crate) fn source_type_arg<'a>(
    kind: &str,
    parsed: &'a ParsedArgs,
) -> Result<Option<&'a str>, CliError> {
    let source = match parsed.value("source-type") {
        Some(raw) => Some(validate_choice("source-type", raw, SOURCE_TYPES)?),
        None => None,
    };
    let values = SOURCE_TYPES.join("|");
    match (kind, source) {
        ("belief", None) => Err(CliError::usage_hint(
            format!("--epistemic-kind belief requires --source-type ({values})"),
            "add --source-type user_stated (or agent_inferred, third_party)",
        )),
        ("fact", Some(_)) => Err(CliError::usage_hint(
            format!(
                "--epistemic-kind fact must not include --source-type; set it only for belief ({values})"
            ),
            "omit --source-type, or set --epistemic-kind belief",
        )),
        (_, source) => Ok(source),
    }
}

#[cfg(test)]
#[path = "cli_write_source_type_tests.rs"]
mod cli_write_source_type_tests;
