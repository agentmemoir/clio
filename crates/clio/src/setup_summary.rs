// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the applied summary `clio setup` prints after
//! the one config write, and for the final note about a stale Compose `.env`.
//!
//! ## Owns
//! - `applied`, the `Applied. N settings written` block
//! - The database line (Postgres parts, or the SQLite file)
//! - The `this machine only` / `encrypted` parenthetical for each endpoint
//! - The key-location line, read from `clio_config::secret_key`
//! - The stale-`.env` note, which never rewrites the file it reports
//!
//! ## Does not own
//! - Writing the config (`setup_config` owns the one write)
//! - Building the values the summary names (`setup_preview` owns the entries)
//! - The preview shown before the write (see `setup_report`)
//!
//! ## Boundary
//!
//! Changes to the applied summary's wording should normally require changes
//! here. Changes to what gets written, or to the preview before it, should not.
//!
//! Keep this module focused on reporting one finished write.
//!
//! # Why no secret can appear here
//!
//! The summary takes [`Written`] and the answers, and it prints a count, an
//! address broken into host and port, two endpoint URLs, and a key *location*.
//! The password inside the address is never read, the API keys are never read,
//! and the entry values themselves are not handed over at all. A summary that
//! received the entry vector could eventually print one by accident; one that
//! receives a count cannot.

use crate::setup_config::Written;
use crate::setup_plan_db::{DatabaseChoice, DatabasePlan};
use crate::setup_services::ServiceRole;
use crate::setup_steps::Answers;
use std::io::Write;
use std::path::Path;

/// The applied summary, printed once the config write has succeeded.
///
/// # Errors
/// The output stream could not be written.
pub(crate) fn applied(
    answers: &Answers,
    written: &Written,
    out: &mut dyn Write,
) -> Result<(), String> {
    writeln!(
        out,
        "  Applied. {} settings written to {}",
        written.fields,
        written.path.display()
    )
    .map_err(io_err)?;
    if let Some(database) = &answers.database {
        writeln!(out, "        Database: {}", database_line(&database.plan)).map_err(io_err)?;
    }
    for (name, service) in [
        ("Embedding", &answers.embed),
        ("Reranking", &answers.rerank),
    ] {
        let Some(service) = service else { continue };
        if service.role == ServiceRole::Skip {
            continue;
        }
        writeln!(
            out,
            "        {name}: {} ({})",
            service.url,
            service_note(&service.url)
        )
        .map_err(io_err)?;
    }
    writeln!(out, "        Encryption key: {}", key_location()).map_err(io_err)?;
    writeln!(
        out,
        "  Run `clio status` to check it, then `clio remember \"...\"`."
    )
    .map_err(io_err)?;
    Ok(())
}

/// The database line: the parts of an address, never the address itself.
fn database_line(plan: &DatabasePlan) -> String {
    let locality = crate::connection_text::database_encryption_locality(&plan.address);
    match plan.choice {
        DatabaseChoice::Sqlite => format!("SQLite file at {}, {locality}", plan.address),
        DatabaseChoice::Managed | DatabaseChoice::Existing => {
            // The address carries the password, so only the host and port are
            // printed. The parser is the store's own, so the two numbers are the
            // ones the driver will dial.
            let shown = match clio_store::parse_connection(&plan.address) {
                Ok(parsed) => format!("{}:{}", parsed.primary_host(), parsed.primary_port()),
                Err(_) => clio_ops::redact_credentials(&plan.address),
            };
            format!("Postgres at {shown}, {locality}")
        }
    }
}

/// The one word that says where a configured endpoint goes.
///
/// A loopback endpoint never leaves the machine, so it is labelled by that. A
/// remote endpoint has to be `https`, because the key travels as a header on
/// every request and the endpoint check refuses a plain one, so `encrypted` is
/// a fact about the address rather than a hope.
fn service_note(url: &str) -> &'static str {
    match clio_types::net::http_target(url) {
        Ok(target) if clio_types::net::is_loopback_host(&target.host) => "this machine only",
        _ => "encrypted",
    }
}

/// Where the encryption key lives, or why that cannot be said.
///
/// Read after the write, because the write is what creates a key when none
/// exists. `None` means no key was created, which is reported as itself rather
/// than as a path that does not exist.
fn key_location() -> String {
    match clio_config::secret_key::location() {
        Ok(Some(location)) => location.to_string(),
        Ok(None) => "(none created)".to_owned(),
        Err(e) => format!("unknown ({})", e.message),
    }
}

/// One note when a Compose root still carries an old `POSTGRES_PASSWORD` line.
///
/// The file is never rewritten: `compose.yml` no longer reads the variable, so
/// the stale value cannot reach the container however Compose resolves the
/// process environment against `.env`. An absent or unreadable file, or a root
/// that cannot be resolved, is silence rather than a second failure after a
/// successful write.
///
/// # Errors
/// The output stream could not be written.
pub(crate) fn stale_env_note(dir: Option<&str>, out: &mut dyn Write) -> Result<(), String> {
    let root = match dir {
        Some(dir) => Path::new(dir).to_path_buf(),
        None => match std::env::current_dir() {
            Ok(dir) => dir,
            Err(_) => return Ok(()),
        },
    };
    let Ok(text) = std::fs::read_to_string(root.join(".env")) else {
        return Ok(());
    };
    if !text
        .lines()
        .any(|line| line.trim_start().starts_with("POSTGRES_PASSWORD="))
    {
        return Ok(());
    }
    writeln!(
        out,
        "  Note: {} still sets POSTGRES_PASSWORD. That value is ignored because compose.yml no \
         longer reads it — the managed Postgres has no password — and the file was not changed.",
        root.join(".env").display()
    )
    .map_err(io_err)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as an argument to map_err"
)]
fn io_err(e: std::io::Error) -> String {
    format!("failed to write the applied summary: {e}")
}

#[cfg(test)]
#[path = "setup_summary_tests.rs"]
mod tests;
