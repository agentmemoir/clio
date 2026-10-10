// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two real probes `clio setup` runs: the
//! store's own Postgres connection test, and the index's own endpoint check.
//!
//! ## Owns
//! - `real_probes`, the pair the wizard is handed
//! - The transcript wording of a successful database check
//! - The refusal of a database without the pgvector extension
//! - Passing the index's transport message through unchanged
//!
//! ## Does not own
//! - The probe signatures (`setup_test_seam` owns those)
//! - Running a probe and printing its result (see `setup_connect`)
//! - Connecting or speaking HTTP: `clio-store` and `clio-index` own both
//!
//! ## Boundary
//!
//! Changes to what a probe checks, or to the wording of the database detail,
//! should normally require changes here. Changes to how a probe's result is
//! rendered should not.
//!
//! Keep this module focused on adapting the two real checks to the wizard seam.
//!
//! # Why the vector check lives here
//!
//! `clio_store::probe_postgres` reports `has_vector` as a value rather than an
//! error, because a report is not a decision. Setup is the caller that decides,
//! and its decision is to refuse: a database without pgvector accepts the
//! connection and then fails at the first memory write, which is exactly the
//! discovery the connection test exists to move forward. One report, one
//! decision, in the module that knows the wizard's rules.

use clio_store::DatabaseReport;

use crate::setup_test_seam::Probes;

/// The two real probes, against the real store and the real transport.
pub(crate) fn real_probes() -> Probes {
    Probes::new(
        |address: &str| {
            let report = clio_store::probe_postgres(address)?;
            database_detail(&report)
        },
        |url: &str, api_key: Option<&str>| {
            clio_index::probe_endpoint(url, api_key).map_err(|e| e.message)
        },
    )
}

/// The one-line detail a successful database check reports.
///
/// # Errors
/// The `vector` extension is missing, naming what Clio needs and how to add it.
pub(crate) fn database_detail(report: &DatabaseReport) -> Result<String, String> {
    if !report.has_vector {
        return Err(
            "this database does not have the pgvector `vector` extension, and Clio cannot store \
             memories without it. Install it with `CREATE EXTENSION vector;` (the standard \
             pgvector-enabled Postgres image, or your provider's vector extension), then test the \
             connection again"
                .to_owned(),
        );
    }
    Ok(format!(
        "connected, PostgreSQL {}, vector extension present",
        report.server_version
    ))
}

#[cfg(test)]
#[path = "setup_probes_tests.rs"]
mod tests;
