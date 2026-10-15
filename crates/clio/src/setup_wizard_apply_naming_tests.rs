// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the apply path's service selection under
//! test: which Compose services each answer set starts, and that a failed
//! Compose start writes no config.
//!
//! ## Owns
//! - Service-naming and start-failure behavior under test for `setup_wizard`
//!
//! ## Does not own
//! - The written config and the password handoff (see `setup_wizard_apply_tests`)
//! - The failure and gate paths (see `setup_wizard_apply_flow_tests`)
//!
//! ## Boundary
//!
//! Changes to which service an answer set starts should normally update tests
//! here. Changes to what is written should not.
//!
//! Keep this module focused on the start call's service list.

use crate::compose_docker::DockerRunner;
use crate::setup_support_tests::{SetupRun, StubDocker, invoke_setup, temp_dir};
use std::path::PathBuf;
use std::process::ExitCode;

/// A hosted endpoint that refuses at once: loopback on a port nothing listens on.
const HOSTED_URL: &str = "https://127.0.0.1:1";
const HOSTED_KEY: &str = "sk-apply-naming-1234567890";

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

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_sqlite_run_with_hosted_models_starts_nothing() {
    let f = fixture("apply-nostart");
    let runner = StubDocker::ok();
    let run = f.run(
        flags(&[
            &["--database", "sqlite"],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ]),
        &runner,
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    assert!(
        runner.invocations().is_empty(),
        "nothing may be started: {:?}",
        runner.invocations()
    );
    f.cleanup();
}

#[test]
fn a_managed_run_with_hosted_models_starts_exactly_postgres() {
    let f = fixture("apply-pgonly");
    let runner = StubDocker::ok();
    let run = f.run(
        flags(&[
            &[
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
            ],
            &HOSTED,
            &SKIPS,
            &["--allow-untested"],
        ]),
        &runner,
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    // The rotation call may appear too, so only the `up` invocation is the
    // service selection: exactly one, and it names postgres alone.
    let invocations = runner.invocations();
    let ups: Vec<&Vec<String>> = invocations
        .iter()
        .filter(|call| call.contains(&"up".to_owned()))
        .collect();
    assert_eq!(ups.len(), 1, "{invocations:?}");
    assert!(ups[0].contains(&"postgres".to_owned()));
    assert!(!runner.ran_arg("embed"), "{invocations:?}");
    assert!(!runner.ran_arg("rerank"), "{invocations:?}");
    f.cleanup();
}

#[test]
fn local_models_add_exactly_the_two_sidecars() {
    let f = fixture("apply-local-models");
    let runner = StubDocker::ok();
    let run = f.run(
        flags(&[
            &["--database", "sqlite"],
            &["--embed", "local", "--rerank", "local", "--extract", "skip"],
            &["--allow-untested"],
        ]),
        &runner,
    );
    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    assert_eq!(runner.invocations().len(), 1, "{:?}", runner.invocations());
    assert!(runner.ran_arg("embed"), "{:?}", runner.invocations());
    assert!(runner.ran_arg("rerank"), "{:?}", runner.invocations());
    assert!(!runner.ran_arg("postgres"), "{:?}", runner.invocations());
    f.cleanup();
}

#[test]
fn a_compose_failure_keeps_the_config_and_says_so() {
    // The config is written before anything runs, so a container that will not start
    // leaves a config the operator agreed to rather than losing an hour of answers
    // to a failure in a sidecar. The message has to say that, because "your settings
    // were saved" and "nothing was written" call for opposite reactions and the
    // operator is the one who has to know which happened.
    let f = fixture("apply-compose-fail");
    let run = f.run(
        flags(&[
            &["--database", "sqlite"],
            &["--embed", "local", "--rerank", "skip", "--extract", "skip"],
        ]),
        &StubDocker::failing_run(),
    );
    assert_eq!(run.code, ExitCode::from(1));
    assert!(
        run.err.contains("Your settings were saved"),
        "the plan's message: {}",
        run.err
    );
    assert!(
        run.err
            .contains("re-running `clio setup` picks up from here"),
        "it has to say what to do next: {}",
        run.err
    );
    assert!(
        f.config.exists(),
        "the config the operator agreed to is kept: {}",
        run.err
    );
    f.cleanup();
}
