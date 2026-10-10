// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for what a managed run hands to Docker and what it
//! leaves in the compose root: no credential at all, and a hand-written `.env`
//! left byte-identical.
//!
//! ## Owns
//! - The absence of a password in the child's environment, its argv, and `.env`
//! - The refusal of `--database-password` on the managed route
//! - The `.env` never-overwrite rule for `setup_wizard`
//!
//! ## Does not own
//! - The written config each database choice produces (see
//!   `setup_wizard_apply_tests`)
//! - The service selection a run starts (see `setup_wizard_apply_naming_tests`)
//! - The failure and gate paths (see `setup_wizard_apply_flow_tests`)
//!
//! ## Boundary
//!
//! Changes to what a run passes to a container should normally update tests
//! here. Changes to what a run writes should not.
//!
//! Keep this module focused on what leaves Clio on its way to Docker.

use crate::compose_docker::DockerRunner;
use crate::setup_support_tests::{SetupRun, StubDocker, env_at, invoke_setup, temp_dir};
use clio_config::Runtime;
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;

/// A hosted endpoint that refuses at once: loopback on a port nothing listens on.
const HOSTED_URL: &str = "https://127.0.0.1:1";
const HOSTED_KEY: &str = "sk-apply-secret-1234567890";

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

/// The managed database's fields, all fixed values Clio would pick on its own.
const MANAGED: [&str; 12] = [
    "--database",
    "managed",
    "--database-host",
    "127.0.0.1",
    "--database-port",
    "34310",
    "--database-name",
    "clio",
    "--database-user",
    "clio",
    "--database-ssl",
    "prefer",
];

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
    fn run(&self, args: Vec<String>, runner: &dyn DockerRunner) -> SetupRun {
        let mut args = args;
        args.push("--config".to_owned());
        args.push(self.config.to_string_lossy().into_owned());
        args.push("--compose-dir".to_owned());
        args.push(self.compose.to_string_lossy().into_owned());
        invoke_setup(
            &args,
            "",
            false,
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

#[test]
fn a_managed_run_hands_docker_no_credential_at_all() {
    let f = fixture("apply-handoff");
    let runner = StubDocker::ok();
    let run = f.run(
        flags(&[&MANAGED, &HOSTED, &SKIPS, &["--allow-untested"]]),
        &runner,
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);

    for (key, _) in runner.last_env() {
        assert!(
            !key.contains("PASSWORD"),
            "the managed database has no password to pass: {key}"
        );
    }
    for call in runner.invocations() {
        assert!(
            !call.iter().any(|arg| arg.contains("PASSWORD")),
            "no credential may reach argv: {call:?}"
        );
    }
    let env_text = std::fs::read_to_string(f.compose.join(".env")).expect(".env written");
    assert!(
        !env_text.contains("POSTGRES_PASSWORD"),
        "no password line belongs in .env: {env_text}"
    );
    assert!(env_text.contains("CLIO_POSTGRES_USER=clio"), "{env_text}");

    let effective = f.effective();
    assert_eq!(effective["backend"], json!("postgres"));
    let address = effective["store"]["database_url"]
        .as_str()
        .expect("a stored address");
    assert!(
        address.starts_with("postgres://clio@"),
        "the managed address carries no password: {address}"
    );
    assert!(address.contains(":34310"), "{address}");
    f.cleanup();
}

#[test]
fn a_managed_run_refuses_a_password_rather_than_ignoring_it() {
    let f = fixture("apply-refuse-pw");
    let run = f.run(
        flags(&[
            &MANAGED,
            &[
                "--database-password",
                "apply-refuse-pw-0123456789",
                "--allow-untested",
            ],
            &HOSTED,
            &SKIPS,
        ]),
        &StubDocker::ok(),
    );
    assert_eq!(run.code, ExitCode::from(2), "out: {}", run.out);
    assert!(
        run.err.contains("--database-password"),
        "the refusal has to name the flag: {}",
        run.err
    );
    assert!(
        !f.config.exists(),
        "nothing is written for a refused run: {}",
        run.err
    );
    f.cleanup();
}

#[test]
fn an_existing_env_is_never_rewritten_and_the_stale_password_is_reported() {
    let f = fixture("apply-env");
    std::fs::create_dir_all(&f.compose).expect("compose dir");
    let env_path = f.compose.join(".env");
    let original =
        "# hand-written by the operator\nPOSTGRES_PASSWORD=legacy-value\nCLIO_POSTGRES_USER=clio\n";
    std::fs::write(&env_path, original).expect("seed .env");

    let run = f.run(
        flags(&[&MANAGED, &HOSTED, &SKIPS, &["--allow-untested"]]),
        &StubDocker::ok(),
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("read .env"),
        original,
        "the operator's file is byte-identical"
    );
    assert!(
        run.out.contains("still sets POSTGRES_PASSWORD"),
        "{}",
        run.out
    );
    assert!(run.out.contains("file was not changed"), "{}", run.out);
    f.cleanup();
}
