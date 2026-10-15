// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that `clio status` and the applied
//! summary `clio setup` prints describe the same store the same way, after a run
//! that really wrote a config.
//!
//! ## Owns
//! - The agreement between the two surfaces, for a real applied run
//!
//! ## Does not own
//! - Either surface's own report/exit tests (see `status_cli_tests` and
//!   `setup_summary_tests`)
//! - The applied summary's wording (see `setup_wizard_apply_flow_tests`)
//!
//! ## Boundary
//!
//! Tests here break when one surface's reading of a store changes and the other
//! does not follow. They never change either surface.
//!
//! Keep this module focused on the two surfaces agreeing, not on either one alone.

use super::*;
use crate::setup_support_tests::{SetupRun, StubDocker, env_at, invoke_setup, temp_dir};
use crate::status_report::ComposeView;
use clio_store::{LocalDevKms, SqliteStore};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

/// A hosted endpoint that refuses at once: loopback on a port nothing listens on.
const HOSTED_URL: &str = "https://127.0.0.1:1";
const HOSTED_KEY: &str = "sk-agree-1234567890";

/// The hosted-embedding flags, so no run under test starts a sidecar.
const HOSTED: [&str; 8] = [
    "--embed",
    "online",
    "--embed-url",
    HOSTED_URL,
    "--embed-model",
    "m",
    "--embed-key",
    HOSTED_KEY,
];

/// Reranking and extraction declined, for runs that are not about either.
const SKIPS: [&str; 4] = ["--rerank", "skip", "--extract", "skip"];

fn flags(parts: &[&[&str]]) -> Vec<String> {
    parts
        .concat()
        .iter()
        .map(|part| (*part).to_owned())
        .collect()
}

/// Status probes that read the config a setup run wrote, and nothing else.
///
/// The point of the test is that `status` resolves the store from the written
/// file, so the environment here names the config and hides everything else,
/// exactly as the setup suite's own runs do. The store is an in-memory SQLite
/// whatever the config says: this asserts the two surfaces *read* the same
/// store, not that a Postgres is reachable.
struct WrittenConfig {
    config: PathBuf,
    dir: crate::setup_support_tests::TempDir,
}

impl WrittenConfig {
    fn env(&self, key: &str) -> Option<String> {
        env_at(&self.config)(key)
    }

    fn setup(&self) -> SetupView {
        let rt = clio_config::Runtime::with_env(&env_at(&self.config));
        build_setup(&rt, Some(&self.config))
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl StatusDeps for WrittenConfig {
    fn env(&self, key: &str) -> Option<String> {
        self.env(key)
    }

    fn open_store(&self, _resolved: &ResolvedDatabase) -> Result<Box<dyn StatusRepos>, AmError> {
        Ok(Box::new(
            SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("memory store"),
        ))
    }

    fn embedder(&self) -> Result<Option<Arc<dyn Embedder>>, AmError> {
        Ok(None)
    }

    fn scan_ports(&self) -> Vec<u16> {
        Vec::new()
    }

    fn compose(&self, _root: &Path) -> ComposeView {
        ComposeView::NotMaterialized
    }

    fn setup(&self) -> SetupView {
        self.setup()
    }

    fn key_location(&self) -> Result<Option<String>, String> {
        // The same function the applied summary reads, so the two are compared
        // on one source rather than on two that could both be wrong. The value
        // it returns is host-dependent and is never asserted on; see
        // `assert_key_agrees`.
        clio_config::secret_key::location()
            .map(|found| found.map(|location| location.to_string()))
            .map_err(|e| e.message)
    }

    fn timestamp(&self) -> String {
        "2026-01-01T00:00:00Z".to_owned()
    }

    fn cwd(&self) -> Option<PathBuf> {
        None
    }
}

/// One temp directory holding the config a run will write.
fn written(tag: &str) -> WrittenConfig {
    let dir = temp_dir(tag);
    let config = dir.join("deployment.json");
    WrittenConfig { config, dir }
}

/// Apply a real flag-driven run and return the config it wrote.
fn apply(written: &WrittenConfig, parts: &[&[&str]]) -> SetupRun {
    let mut args = flags(parts);
    args.push("--config".to_owned());
    args.push(written.config.to_string_lossy().into_owned());
    args.push("--compose-dir".to_owned());
    args.push(written.dir.join("compose").to_string_lossy().into_owned());
    invoke_setup(
        &args,
        "",
        false,
        &written.config,
        &StubDocker::ok(),
        &crate::setup_test_seam::Probes::ok(),
    )
}

/// The `Database:` line the applied summary printed, without its indent.
fn summary_database_line(summary: &str) -> String {
    summary
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Database: "))
        .unwrap_or_else(|| panic!("the summary names the database:\n{summary}"))
        .to_owned()
}

/// The `Encryption key:` line the applied summary printed, without its indent.
fn summary_key_line(summary: &str) -> String {
    summary
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("Encryption key: "))
        .unwrap_or_else(|| panic!("the summary names the key:\n{summary}"))
        .to_owned()
}

/// Assert the two surfaces agree about the encryption key.
///
/// The *value* is deliberately not compared. `clio_config::secret_key::location`
/// reads the host's key store, which is a macOS Keychain on a laptop, a Secret
/// Service on a Linux runner, and a key file when the operator named one — so a
/// test that pinned the value passed locally and failed on CI, which is the whole
/// class of bug this assertion exists to prevent.
///
/// What is pinned is that both surfaces report a key location at all, and that
/// neither reports one as absent while the other reports it. That is the part
/// that can disagree between the two, and it holds on every machine.
fn assert_key_agrees(summary: &str, database: &DatabaseView) {
    let line = summary_key_line(summary);
    let value = line
        .strip_prefix("Encryption key: ")
        .expect("the prefix is the line's own")
        .trim();
    assert!(!value.is_empty(), "the line carries a value: {line}");

    // Three states, not two. A readable location is one string; a read that
    // failed is reported as a reason by the summary and as `key_error` by status,
    // so those two shapes differ by design; and a machine where no key has been
    // created yet is a third shape, which reads as a location to anything that
    // only asks "is this an error?". It is not: status reports no location for
    // it either, and comparing the two as if the value were a path fails on any
    // machine that has never run a setup.
    let created = !value.starts_with("(none created)");
    let readable = !value.starts_with("unknown (");
    assert_eq!(
        readable && created,
        database.key_location.is_some(),
        "one surface reports a location and the other does not: summary {line:?}, status {:?}",
        database.key_location
    );
    if readable && created {
        assert_eq!(
            value,
            database
                .key_location
                .as_deref()
                .expect("readable on both sides"),
            "both surfaces name the same location: {line}"
        );
    } else if !readable {
        // Both must be reporting the same failed read, so the reason has to be
        // present on both sides even though it is shaped differently.
        assert!(
            database.key_error.is_some(),
            "a failed read is reported by both: summary {line:?}"
        );
    }
}

/// Read the config a run wrote, to prove both surfaces resolved the same one.
fn written_document(written: &WrittenConfig) -> serde_json::Value {
    let text = std::fs::read_to_string(&written.config).expect("the run wrote a config");
    serde_json::from_str(&text).expect("json")
}

#[test]
fn status_agrees_with_the_applied_summary_about_a_remote_postgres() {
    let written = written("agree-remote");
    let address = "postgres://clio:pw@db.example.com:5432/clio?sslmode=verify-full";
    let run = apply(
        &written,
        &[
            &[
                "--database",
                "existing",
                "--database-connect",
                "pasted",
                "--database-url",
                address,
            ],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ],
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);

    let summary = &run.out;
    let (report, _) = build(&[], &written).expect("ok build");

    // The backend and the address travel as a pair, and both surfaces resolved
    // the pair the run wrote rather than a default.
    assert_eq!(report.database.backend, "postgres");
    assert_eq!(written_document(&written)["backend"], json!("postgres"));
    assert_eq!(
        report.database.url,
        "postgres://***@db.example.com:5432/clio?sslmode=verify-full"
    );

    // The encryption sentence is the one fact both surfaces print about the same
    // address, so it must be the same sentence.
    let encryption = "encrypted and verified, remote";
    assert_eq!(report.database.encryption, encryption);
    assert!(
        summary_database_line(summary).ends_with(encryption),
        "the summary and status must describe the address the same way: {}",
        summary_database_line(summary)
    );

    // And the summary names the same host and port status resolved.
    assert!(
        summary_database_line(summary).contains("db.example.com:5432"),
        "{}",
        summary_database_line(summary)
    );

    // The key location is read by both from the same function, so the two
    // surfaces must not disagree about whether there is one.
    assert_key_agrees(summary, &report.database);
    written.cleanup();
}

#[test]
fn status_agrees_with_the_applied_summary_about_a_sqlite_file() {
    let written = written("agree-sqlite");
    let db = written.dir.join("memories.db");
    let db_text = db.to_string_lossy().into_owned();
    let run = apply(
        &written,
        &[
            &["--database", "sqlite", "--db-path", &db_text],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ],
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);

    let summary = &run.out;
    let (report, _) = build(&[], &written).expect("ok build");

    assert_eq!(report.database.backend, "sqlite");
    assert_eq!(written_document(&written)["backend"], json!("sqlite"));

    // A file is the store, so both surfaces must name the same path.
    assert_eq!(report.database.url, db_text);
    assert!(
        summary_database_line(summary).contains(&db_text),
        "{}",
        summary_database_line(summary)
    );

    // And a file is not encrypted, on either reading.
    let encryption = "not encrypted, this machine only";
    assert_eq!(report.database.encryption, encryption);
    assert!(
        summary_database_line(summary).ends_with(encryption),
        "the summary and status must describe the file the same way: {}",
        summary_database_line(summary)
    );
    written.cleanup();
}

/// A `DatabaseView` with only the key fields set, for the shape tests.
fn key_view(location: Option<&str>, error: Option<&str>) -> DatabaseView {
    DatabaseView {
        backend: "postgres".to_owned(),
        url: "postgres://***@db.example.com:5432/clio?sslmode=verify-full".to_owned(),
        encryption: "encrypted and verified, remote".to_owned(),
        key_location: location.map(str::to_owned),
        key_error: error.map(str::to_owned),
    }
}

#[test]
fn a_key_read_that_fails_is_agreement_not_disagreement() {
    // The exact shape the Linux runner produced: the key store is unavailable,
    // so the summary prints the reason inline and status reports it as
    // `key_error` with no location. An earlier version of this suite compared
    // the two values and failed here, having passed on a machine with a
    // Keychain. Pinned so the same comparison cannot come back.
    let summary = "  Applied. 6 settings written to /tmp/x/deployment.json\n        \
                   Database: Postgres at db.example.com:5432, encrypted and verified, remote\n        \
                   Encryption key: unknown (cannot read from the Clio encryption key \
                   in Linux Secret Service: the platform store failed)\n";
    assert_key_agrees(
        summary,
        &key_view(
            None,
            Some(
                "cannot read from the Clio encryption key in Linux Secret Service: \
                  the platform store failed",
            ),
        ),
    );
}

#[test]
fn a_key_read_that_succeeds_is_compared_on_both_sides() {
    // The macOS shape: both surfaces name the same location, so the two strings
    // are compared here because both are pinned to the same value.
    let summary = "  Encryption key: macOS Keychain\n";
    assert_key_agrees(summary, &key_view(Some("macOS Keychain"), None));
}

#[test]
fn one_surface_reporting_a_location_and_the_other_not_is_disagreement() {
    // The case the assertion exists for: the summary names a key and status does
    // not, which is the bug this suite was written to catch.
    let summary = "  Encryption key: macOS Keychain\n";
    let view = key_view(None, None);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_key_agrees(summary, &view);
    }));
    assert!(outcome.is_err(), "a location on one side only must fail");
}
