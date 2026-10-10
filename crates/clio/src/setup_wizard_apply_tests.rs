// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the apply path's scripted runs: the config a
//! flag-driven run writes for each database choice, and the stale file path a
//! switch to Postgres removes.
//!
//! ## Owns
//! - Flag-driven apply behavior under test for `setup_wizard`
//!
//! ## Does not own
//! - The managed password's handoff to Docker and the `.env` rule (see
//!   `setup_wizard_apply_secret_tests`)
//! - Which services an answer set starts (see `setup_wizard_apply_naming_tests`)
//! - The stop, rotation, recreate, switch, and summary runs (see
//!   `setup_wizard_apply_flow_tests`)
//! - The questions themselves (the stage 5 suites cover them)
//!
//! ## Boundary
//!
//! Changes to what a flag-driven run writes should normally update tests here.
//! Changes to the failure handling should not.
//!
//! Keep this module focused on the happy path's written config.

use crate::compose_docker::DockerRunner;
use crate::setup_support_tests::{
    SetupRun, StubDocker, TEST_PG_HOST, TEST_PG_NAME, TEST_PG_PASSWORD, TEST_PG_PORT, TEST_PG_USER,
    env_at, invoke_setup, temp_dir,
};
use crate::setup_wizard::{LOCAL_EMBED_MODEL, LOCAL_EMBED_URL};
use clio_config::Runtime;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

/// A hosted endpoint that refuses at once: loopback on a port nothing listens on.
const HOSTED_URL: &str = "https://127.0.0.1:1";
const HOSTED_KEY: &str = "sk-apply-test-1234567890";

/// The secret's path out of Clio and into Docker, in their own module.
///
/// Split out rather than declared in `setup_wizard`, because that file is at the
/// workspace's line limit and a fourth `#[path]` there would put it over.
#[cfg(test)]
#[path = "setup_wizard_apply_tests_secret_tests.rs"]
mod secret_tests;

/// The hosted-embedding flags, used by every run that should start no sidecar.
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

/// One temp directory with the config and compose paths a run uses.
struct Fixture {
    dir: PathBuf,
    config: PathBuf,
    compose: PathBuf,
}

fn fixture(tag: &str) -> Fixture {
    let dir = temp_dir(tag);
    let config = dir.join("deployment.json");
    let compose = dir.join("compose");
    Fixture {
        dir,
        config,
        compose,
    }
}

impl Fixture {
    fn run(
        &self,
        args: Vec<String>,
        input: &str,
        tty: bool,
        runner: &dyn DockerRunner,
    ) -> SetupRun {
        let mut args = args;
        args.push("--config".to_owned());
        args.push(self.config.to_string_lossy().into_owned());
        args.push("--compose-dir".to_owned());
        args.push(self.compose.to_string_lossy().into_owned());
        self.run_with(
            args,
            input,
            tty,
            runner,
            &crate::setup_test_seam::Probes::ok(),
        )
    }

    /// The same run with probes the scenario chooses.
    ///
    /// A refusal on a closed port is retried, so a suite that only asserts what a
    /// run wrote takes the succeeding probe and the one scenario that is about the
    /// waiver takes the refusal.
    fn run_with(
        &self,
        args: Vec<String>,
        input: &str,
        tty: bool,
        runner: &dyn DockerRunner,
        probes: &crate::setup_test_seam::Probes,
    ) -> SetupRun {
        let mut args = args;
        args.push("--config".to_owned());
        args.push(self.config.to_string_lossy().into_owned());
        args.push("--compose-dir".to_owned());
        args.push(self.compose.to_string_lossy().into_owned());
        invoke_setup(&args, input, tty, &self.config, runner, probes)
    }

    fn effective(&self) -> serde_json::Value {
        Runtime::with_env(&env_at(&self.config)).effective_raw()
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The local Compose database's address, as the tests' existing-database path.
fn test_pg_address() -> String {
    format!(
        "postgres://{TEST_PG_USER}:{TEST_PG_PASSWORD}@{TEST_PG_HOST}:{TEST_PG_PORT}/{TEST_PG_NAME}?sslmode=prefer"
    )
}

#[test]
fn a_sqlite_run_writes_the_backend_the_path_and_the_service() {
    let f = fixture("apply-sqlite");
    let db = f.dir.join("memories.db");
    let db_text = db.to_string_lossy().into_owned();
    let run = f.run(
        flags(&[
            &["--database", "sqlite", "--db-path", &db_text],
            &["--embed", "local", "--rerank", "skip", "--extract", "skip"],
            // The local sidecar is a `StubDocker`, so nothing is really listening
            // on its URL and the post-start test cannot pass. Every other run in
            // this file waives it for the same reason; without the waiver a
            // non-interactive run stops and fails on a service the test never
            // started, and it passes only on a machine that happens to run one.
            &["--allow-untested"],
        ]),
        "",
        false,
        &StubDocker::ok(),
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&f.config).expect("config written"))
            .expect("json");
    let keys: BTreeSet<&str> = raw
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "backend",
            "store.database_url",
            "store.sqlite_path",
            "embed.provider",
            "embed.url",
            "embed.model",
        ]),
        "the paired keys and the file path, and nothing stale"
    );

    let effective = f.effective();
    assert_eq!(effective["backend"], json!("sqlite"));
    assert_eq!(effective["store"]["database_url"], json!(db_text));
    assert_eq!(effective["store"]["sqlite_path"], json!(db_text));
    assert_eq!(effective["embed"]["provider"], json!("tei"));
    assert_eq!(effective["embed"]["url"], json!(LOCAL_EMBED_URL));
    assert_eq!(effective["embed"]["model"], json!(LOCAL_EMBED_MODEL));
    assert!(
        run.out.contains("Applied. 6 settings written to"),
        "{}",
        run.out
    );
    f.cleanup();
}

#[test]
fn an_existing_run_keeps_the_address_exactly() {
    let f = fixture("apply-existing");
    let address = test_pg_address();
    let run = f.run(
        flags(&[
            &[
                "--database",
                "existing",
                "--database-connect",
                "pasted",
                "--database-url",
                &address,
            ],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ]),
        "",
        false,
        &StubDocker::ok(),
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);

    let effective = f.effective();
    assert_eq!(effective["backend"], json!("postgres"));
    assert_eq!(effective["store"]["database_url"], json!(address));
    assert_eq!(effective["embed"]["provider"], json!("openai"));
    assert_eq!(effective["credentials"]["embed_api_key"], json!(HOSTED_KEY));
    assert!(
        !run.out.contains(HOSTED_KEY),
        "the key must not be printed: {}",
        run.out
    );
    f.cleanup();
}

#[test]
fn allow_untested_waives_a_typed_connection_failure_at_the_question() {
    let f = fixture("apply-typed-untested");
    let probes = crate::setup_support_tests::failing_database_probe();
    let run = f.run_with(
        flags(&[
            &[
                "--database",
                "existing",
                "--database-connect",
                "typed",
                "--database-host",
                "127.0.0.1",
                "--database-port",
                "34310",
                "--database-name",
                "clio",
                "--database-user",
                "clio",
                "--database-password",
                "definitely-wrong-0123456789",
                "--database-ssl",
                "prefer",
            ],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ]),
        "",
        false,
        &StubDocker::ok(),
        &probes,
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    assert!(
        run.out.contains("Clio will not work until this is fixed"),
        "the waiver warning is printed: {}",
        run.out
    );
    assert!(
        run.out.contains("not tested"),
        "the preview says the database was not tested: {}",
        run.out
    );
    let effective = f.effective();
    assert_eq!(effective["backend"], json!("postgres"));
    assert!(
        effective["store"]["database_url"]
            .as_str()
            .expect("a stored address")
            .contains("127.0.0.1:34310"),
        "the typed address is written: {effective}"
    );
    f.cleanup();
}

#[test]
fn switching_from_sqlite_to_postgres_removes_the_stale_file_path() {
    let f = fixture("apply-switch-path");
    let db = f.dir.join("memories.db");
    let db_text = db.to_string_lossy().into_owned();
    let first = f.run(
        flags(&[
            &["--database", "sqlite", "--db-path", &db_text],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ]),
        "",
        false,
        &StubDocker::ok(),
    );
    assert_eq!(first.code, ExitCode::SUCCESS, "err: {}", first.err);

    let address = test_pg_address();
    let second = f.run(
        flags(&[
            &[
                "--database",
                "existing",
                "--database-connect",
                "pasted",
                "--database-url",
                &address,
            ],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ]),
        "",
        false,
        &StubDocker::ok(),
    );
    assert_eq!(second.code, ExitCode::SUCCESS, "err: {}", second.err);

    let effective = f.effective();
    assert_eq!(effective["backend"], json!("postgres"));
    assert_eq!(effective["store"]["database_url"], json!(address));
    // The merged document always carries a default `store.sqlite_path`, so the
    // stale value is checked in the file the run wrote: the key must be gone.
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&f.config).expect("read")).expect("json");
    assert!(
        raw.get("store.sqlite_path").is_none(),
        "the stale path must not survive: {raw}"
    );
    assert_ne!(
        effective["store"]["sqlite_path"],
        json!(db_text),
        "{effective}"
    );
    f.cleanup();
}

#[test]
fn a_multi_host_run_records_the_sync_key_store_beside_the_key_file() {
    let f = fixture("apply-sync-key");
    // The sync question is in play only when the effective config does not omit
    // sync, and the config has to exist before the run reads it.
    std::fs::write(&f.config, r#"{"sync":{"omit":false}}"#).expect("seed the effective config");
    let db = f.dir.join("memories.db");
    let db_text = db.to_string_lossy().into_owned();
    let run = f.run(
        flags(&[
            &["--database", "sqlite", "--db-path", &db_text],
            &["--embed", "local", "--rerank", "skip", "--extract", "skip"],
            &["--sync-store", "keystore", "--allow-untested"],
        ]),
        "",
        false,
        &StubDocker::ok(),
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);

    let marker = f.dir.join("sync-key-source");
    assert_eq!(
        std::fs::read_to_string(&marker).expect("the sync key store is recorded"),
        "keystore\n"
    );
    // The record names which store holds the sealing key, so it is owner-only
    // like the key file. The shape is asserted rather than the exact value: a
    // stricter umask must not fail the test, and a looser one must not pass it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&marker)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "the record grants nothing to group or other: {mode:o}"
        );
    }
    // The sync key's record is its own file; the encryption key's is untouched.
    assert_eq!(
        std::fs::read_to_string(f.dir.join("key-source")).expect("the data key record"),
        "file\n"
    );
    f.cleanup();
}

#[test]
fn a_provider_flag_reaches_the_config_the_run_writes() {
    // The flag is offered in `setup --help` beside the other service flags, so
    // the value it names has to be the one the file carries. A run that wrote
    // the role default instead reported success over a provider the operator
    // never chose, and the wrong one surfaced later behind the endpoint.
    let f = fixture("apply-provider-flag");
    let address = test_pg_address();
    let run = f.run(
        flags(&[
            &[
                "--database",
                "existing",
                "--database-connect",
                "pasted",
                "--database-url",
                &address,
            ],
            &HOSTED,
            // The endpoint on a closed port is unreachable, so the probe fails and
            // the waiver keeps the answers: what is under test is the provider the
            // run records, not whether the endpoint answered.
            &["--embed-provider", "tei", "--allow-untested"],
            &SKIPS,
        ]),
        "",
        false,
        &StubDocker::ok(),
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);

    let effective = f.effective();
    assert_eq!(effective["embed"]["provider"], json!("tei"), "{effective}");
    assert_eq!(effective["embed"]["url"], json!(HOSTED_URL), "{effective}");
    f.cleanup();
}
