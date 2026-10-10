// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the write half of `clio setup`: the values a
//! managed database needs, the single config write, and the applied summary.
//!
//! ## Owns
//! - `managed_env`: the chosen port a managed database needs
//! - `apply`: `setup_config::write` exactly once, then the summary
//!
//! ## Does not own
//! - The questions and the preview (see `setup_steps` and `setup_review`)
//! - Starting and checking the containers (see `setup_verify`)
//! - The config entries themselves (`setup_preview` builds them)
//! - The applied summary's wording (see `setup_summary`)
//!
//! ## Boundary
//!
//! Changes to what the config write needs, or to the write itself, should
//! normally require changes here. Changes to a question, a preview line, or a
//! Docker call should not.
//!
//! Keep this module focused on the one write and the values that reach it.
//!
//! # Why nothing is written before the preview
//!
//! The preview is the operator's last chance to see what will change, and the
//! switch gate in front of it is only meaningful if the write has not happened
//! yet. So the ordering is fixed: answers, the preview, its gate, then exactly
//! one write, and only then anything that touches Docker.
//!
//! # Why no secret reaches the child process
//!
//! The managed database has no password, so there is nothing to hand to Docker.
//! The only value that travels is the port, which is not secret. The hosted
//! API keys never reach Docker either — Compose never sees them.

use crate::setup_config::{self, Written};
use crate::setup_plan_db::DatabaseChoice;
use crate::setup_steps::Answers;
use clio_config::Runtime;
use std::io::Write;
use std::path::Path;

/// Local sidecar endpoints and models, matching `.env.example` / `compose.yml`.
pub(crate) const LOCAL_EMBED_URL: &str = "http://127.0.0.1:34311";
/// Local rerank sidecar endpoint.
pub(crate) const LOCAL_RERANK_URL: &str = "http://127.0.0.1:34312";
/// Default local embedding model (the compose sidecar default).
pub(crate) const LOCAL_EMBED_MODEL: &str = "BAAI/bge-small-en-v1.5";
/// Default local rerank model (the compose sidecar default).
pub(crate) const LOCAL_RERANK_MODEL: &str = "onnx-community/gte-multilingual-reranker-base";

/// The environment a managed database's chosen port travels in, or nothing.
///
/// Only the managed choice has one: a port Clio chose. Every other choice passes
/// nothing and Compose's own defaults apply.
///
/// The port travels here rather than into `.env` because `compose.yml` reads
/// `${CLIO_POSTGRES_PORT:-34310}` and the `.env` in the compose root is never
/// overwritten. A chosen port that is not 34310 has to reach the child process
/// as an environment variable or the container would publish a different port
/// from the one the stored address names.
///
/// There is no password to pass. The managed database has none: `compose.yml`
/// asks the image for trust authentication and publishes the port on loopback
/// only, so the bind is the boundary rather than a credential.
#[must_use]
pub(crate) fn managed_env(answers: &Answers) -> Vec<(String, String)> {
    let Some(database) = answers.database.as_ref() else {
        return Vec::new();
    };
    if database.plan.choice != DatabaseChoice::Managed {
        return Vec::new();
    }
    managed_port(database).map_or_else(Vec::new, |port| vec![(MANAGED_PORT_VAR.to_owned(), port)])
}

/// The port a managed plan names, read back out of its own address.
///
/// Read from the address rather than held beside it, so the value handed to
/// Compose and the value the store will dial cannot be two different numbers.
fn managed_port(database: &crate::setup_flows_db::DatabaseAnswers) -> Option<String> {
    clio_store::parse_connection(&database.plan.address)
        .ok()
        .map(|parsed| parsed.primary_port().to_string())
}

/// The environment variable Compose reads for the managed port.
///
/// The same name the generated `.env` writes, because compose substitutes the
/// host side of a variable: the key here and the key there are one spelling. It
/// is a name, not a port, so the constant is named for what it holds.
pub(crate) const MANAGED_PORT_VAR: &str = "CLIO_POSTGRES_PORT";

/// Write the config exactly once and print the applied summary.
///
/// The key store is recorded first, before the config is sealed. Sealing loads
/// the key, and the key's store is what the record decides, so recording it
/// afterwards would mean the seal had already picked a store the operator had
/// not chosen yet.
///
/// The entries come from the same [`crate::setup_preview::planned_entries`] the
/// preview drew, so the preview and the write cannot disagree.
///
/// # Errors
/// A value the config layer refuses, a failed backup, or a failed write. The
/// stream failure stops the summary, not the write, which has already finished.
pub(crate) fn apply(
    answers: &Answers,
    rt: &mut Runtime,
    config_path: &Path,
    out: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Written, String> {
    if let Some(source) = answers.key_source {
        // An already-recorded choice wins, so a second setup on the same machine
        // reports the store that is in use rather than the one just highlighted.
        //
        // The environment is the caller's, not the process's. This writes a file
        // beside the key, so resolving the path from the real environment would put
        // a marker in the data directory of whoever ran the test.
        let recorded = clio_config::secret_key_source::record(&|key| env(key), source);
        recorded.map_err(|err| err.message)?;
    }
    if let Some(source) = answers.sync_key_source {
        // The same one-time record for the sync key's own marker, in the same
        // injected environment.
        let recorded = clio_config::sync_key::record_source(&|key| env(key), source);
        recorded.map_err(|err| err.message)?;
    }
    let entries = crate::setup_preview::planned_entries(answers);
    let written = setup_config::write(rt, config_path, &entries)?;
    crate::setup_summary::applied(answers, &written, out)?;
    Ok(written)
}

#[cfg(test)]
#[path = "setup_wizard_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "setup_wizard_apply_tests.rs"]
mod apply_tests;

#[cfg(test)]
#[path = "setup_wizard_apply_naming_tests.rs"]
mod apply_naming_tests;

#[cfg(test)]
#[path = "setup_wizard_apply_flow_tests.rs"]
mod apply_flow_tests;

#[cfg(test)]
#[path = "setup_wizard_apply_switch_tests.rs"]
mod apply_switch_tests;
