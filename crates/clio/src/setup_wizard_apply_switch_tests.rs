// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the typed `switch` gate under test: a
//! database change over a store with rows, over an unreadable store, and the
//! refusal that leaves everything alone.
//!
//! ## Owns
//! - Switch-gate behavior under test for `setup_wizard` and `setup_review`
//!
//! ## Does not own
//! - The store-state unit decisions (see `setup_store_state_tests`)
//! - The prompt's own wording (see `setup_report_amend_tests`)
//! - The failure and summary paths (see `setup_wizard_apply_flow_tests`)
//!
//! ## Boundary
//!
//! Changes to when the typed word is required should normally update tests
//! here. Changes to what the prompt says should not.
//!
//! Keep this module focused on the switch gate.

use crate::compose_docker::DockerRunner;
use crate::setup_support_tests::{
    SetupRun, StubDocker, env_at, invoke_setup, seed_sqlite_store, temp_dir,
};
use clio_config::Runtime;
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;

const HOSTED_URL: &str = "https://127.0.0.1:1";
const HOSTED_KEY: &str = "sk-apply-switch-1234567890";

fn flags(parts: &[&[&str]]) -> Vec<String> {
    parts
        .concat()
        .iter()
        .map(|part| (*part).to_owned())
        .collect()
}

/// One temp directory with the config and compose paths a run uses.
struct Fixture {
    dir: crate::setup_support_tests::TempDir,
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
        invoke_setup(
            &args,
            input,
            tty,
            &self.config,
            runner,
            &crate::setup_test_seam::Probes::ok(),
        )
    }

    fn effective(&self) -> serde_json::Value {
        Runtime::with_env(&env_at(&self.config)).effective_raw()
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The local Compose database's address, as the tests' existing-database path.
fn test_pg_address() -> &'static str {
    "postgres://clio:clio@127.0.0.1:34310/clio?sslmode=prefer"
}

/// A non-interactive existing-Postgres run, with the probe waived.
fn existing_pasted(address: &str) -> Vec<String> {
    flags(&[
        &[
            "--database",
            "existing",
            "--database-connect",
            "pasted",
            "--database-url",
            address,
        ],
        &[
            "--embed",
            "online",
            "--embed-url",
            HOSTED_URL,
            "--embed-model",
            "m",
            "--embed-key",
            HOSTED_KEY,
        ],
        &["--rerank", "skip", "--extract", "skip"],
        &["--allow-untested"],
    ])
}

/// A non-interactive SQLite run that writes the config the gate then reads.
fn sqlite_config_run(f: &Fixture, db: &str) -> SetupRun {
    f.run(
        flags(&[
            &["--database", "sqlite", "--db-path", db],
            &[
                "--embed",
                "online",
                "--embed-url",
                HOSTED_URL,
                "--embed-model",
                "m",
                "--embed-key",
                HOSTED_KEY,
            ],
            &["--rerank", "skip", "--extract", "skip"],
            &["--allow-untested"],
        ]),
        "",
        false,
        &StubDocker::ok(),
    )
}

#[test]
fn switching_over_a_store_with_rows_needs_the_typed_switch() {
    let f = fixture("flow-switch");
    let old = f.dir.join("old.db");
    let old_text = old.to_string_lossy().into_owned();
    let first = sqlite_config_run(&f, &old_text);
    assert_eq!(first.code, ExitCode::SUCCESS, "err: {}", first.err);
    seed_sqlite_store(&old, &[("i1", "s1"), ("i2", "s1"), ("i3", "s2")]);
    let before = std::fs::read(&old).expect("the seeded store");

    let mut args = existing_pasted(test_pg_address());
    args.push("--interactive".to_owned());
    // `y` agrees at the apply gate; then an empty line re-asks at the typed
    // confirmation, and the word accepts on the second attempt.
    let run = f.run(args, "y\n\nswitch\n", true, &StubDocker::ok());

    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    assert!(run.out.contains("3 memories"), "{}", run.out);
    assert!(run.out.contains("2 subjects"), "{}", run.out);
    assert!(
        run.out.contains("Nothing is deleted and nothing is copied"),
        "{}",
        run.out
    );
    assert!(run.out.contains("`switch` was not typed"), "{}", run.out);
    assert_eq!(
        std::fs::read(&old).expect("the store is still there"),
        before,
        "the old store file is byte-identical"
    );
    assert_eq!(f.effective()["backend"], json!("postgres"));
    f.cleanup();
}

#[test]
fn an_untyped_switch_leaves_the_config_and_the_store_alone() {
    let f = fixture("flow-switch-refuse");
    let old = f.dir.join("old.db");
    let old_text = old.to_string_lossy().into_owned();
    let first = sqlite_config_run(&f, &old_text);
    assert_eq!(first.code, ExitCode::SUCCESS, "err: {}", first.err);
    seed_sqlite_store(&old, &[("i1", "s1"), ("i2", "s2")]);
    let config_before = std::fs::read_to_string(&f.config).expect("config");

    let mut args = existing_pasted(test_pg_address());
    args.push("--interactive".to_owned());
    let run = f.run(args, "y\nno\nok\nsure\n", true, &StubDocker::ok());

    assert_eq!(run.code, ExitCode::from(1));
    assert!(run.err.contains("nothing was changed"), "{}", run.err);
    assert_eq!(
        std::fs::read_to_string(&f.config).expect("config"),
        config_before,
        "the config is untouched"
    );
    assert_eq!(f.effective()["backend"], json!("sqlite"));
    f.cleanup();
}

#[test]
fn an_unreadable_current_store_still_requires_the_switch() {
    let f = fixture("flow-switch-unreadable");
    let missing = f.dir.join("never-created.db");
    let missing_text = missing.to_string_lossy().into_owned();
    let first = sqlite_config_run(&f, &missing_text);
    assert_eq!(first.code, ExitCode::SUCCESS, "err: {}", first.err);

    let mut args = existing_pasted(test_pg_address());
    args.push("--interactive".to_owned());
    let run = f.run(args, "y\nswitch\n", true, &StubDocker::ok());

    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    assert!(
        run.out
            .contains("could not read the database it is using now"),
        "the reason is printed: {}",
        run.out
    );
    assert!(
        run.out.contains("Type `switch`"),
        "the typed word is still required: {}",
        run.out
    );
    f.cleanup();
}
