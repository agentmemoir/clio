// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the apply path's failure runs: the stop after
//! a failed test, the rotation and recreate paths, and the applied summary's
//! exact lines.
//!
//! ## Owns
//! - Failure handling and summary behavior under test for `setup_wizard` and
//!   `setup_summary`
//!
//! ## Does not own
//! - The happy-path writes and service naming (see `setup_wizard_apply_tests`)
//! - The typed `switch` gate (see `setup_wizard_apply_switch_tests`)
//! - The store-state unit decisions (see `setup_store_state_tests`)
//!
//! ## Boundary
//!
//! Changes to what a failed apply stops, rotates, or requires should normally
//! update tests here. Changes to the happy path should not.
//!
//! Keep this module focused on the paths that can destroy or abandon work.

use crate::compose_docker::DockerRunner;
use crate::setup_support_tests::{
    SetupRun, StubDocker, failing_database_probe, failing_endpoint_probe,
    fails_once_database_probe, invoke_setup, temp_dir,
};
use crate::setup_wizard::LOCAL_EMBED_URL;
use std::path::PathBuf;
use std::process::ExitCode;

fn flags(parts: &[&[&str]]) -> Vec<String> {
    parts
        .concat()
        .iter()
        .map(|part| (*part).to_owned())
        .collect()
}

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
    /// A run whose database probe refuses, which is what most of this file wants:
    /// the failure path, the rotation, and the question that follows it.
    fn run(
        &self,
        args: Vec<String>,
        input: &str,
        tty: bool,
        runner: &dyn DockerRunner,
    ) -> SetupRun {
        self.run_with(args, input, tty, runner, &failing_database_probe())
    }

    /// A run whose database probe answers.
    ///
    /// Separate from [`Fixture::run`] because the two halves of this file need
    /// opposite worlds: a test about what happens when the connection fails must
    /// get a refusal, and a test about what happens after it succeeds must get a
    /// success. Sharing one default meant whichever half was written last decided
    /// the other's verdict.
    fn run_healthy(
        &self,
        args: Vec<String>,
        input: &str,
        tty: bool,
        runner: &dyn DockerRunner,
    ) -> SetupRun {
        self.run_with(
            args,
            input,
            tty,
            runner,
            &crate::setup_test_seam::Probes::ok(),
        )
    }

    /// The same run with probes the scenario chooses.
    fn run_with(
        &self,
        mut args: Vec<String>,
        input: &str,
        tty: bool,
        runner: &dyn DockerRunner,
        probes: &crate::setup_test_seam::Probes,
    ) -> SetupRun {
        args.push("--config".to_owned());
        args.push(self.config.to_string_lossy().into_owned());
        args.push("--compose-dir".to_owned());
        args.push(self.compose.to_string_lossy().into_owned());
        invoke_setup(&args, input, tty, &self.config, runner, probes)
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A managed plan whose database test is the one that fails.
///
/// There is no password to break any more. The managed database has none, so the
/// only way a managed connection fails is that the container is not answering —
/// and the probe stub is what makes it fail here.
fn broken_managed() -> Vec<String> {
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
        &["--embed", "local", "--rerank", "skip", "--extract", "skip"],
        // The key store is asked on any run that has never chosen, so every
        // fixture in this file answers it. The flag is the same answer as typing
        // it, and it keeps the scripts about the questions under test rather than
        // about the one in front of them.
        &["--key-store", "file"],
    ])
}

/// A SQLite install with a local sidecar, so the only thing that can fail is the
/// sidecar. A pasted or managed database fails earlier or takes the rotation path,
/// and neither reaches the question this is about.
fn broken_sidecar() -> Vec<String> {
    flags(&[
        &["--database", "sqlite"],
        &["--embed", "local", "--rerank", "skip", "--extract", "skip"],
        &["--key-store", "file"],
    ])
}

fn contains_call(calls: &[Vec<String>], needle: &str) -> bool {
    calls.iter().any(|call| call.contains(&needle.to_owned()))
}

#[test]
fn leaving_at_the_preview_touches_no_container_and_writes_nothing() {
    // Docker is not touched until after the operator has agreed. This run is
    // walked to the gate and then stopped, so the assertion is that the runner was
    // never called at all — which is stronger than "nothing was left running", and
    // is the property the ordering exists to provide. The old version of this test
    // asserted that a started service was stopped, which described the old order.
    let f = fixture("flow-stop");
    let runner = StubDocker::ok();
    let mut args = broken_managed();
    args.extend(["--allow-untested".to_owned(), "--interactive".to_owned()]);
    // The gate is a yes-or-no, so the way out of it is the prompt's own cancel
    // rather than a word, and a reader cannot produce one. The run therefore ends on
    // a usage stop with input exhausted, and the three properties under test hold on
    // that stop as much as on a clean exit: no container was started, no config was
    // written, and the operator is told why on stderr.
    let run = f.run(args, "", true, &runner);

    assert_eq!(run.code, ExitCode::from(2), "err: {}", run.err);
    assert!(
        runner.invocations().is_empty(),
        "nothing may be started before the operator agrees: {:?}",
        runner.invocations()
    );
    assert!(!f.config.exists(), "nothing may be written");
    assert!(
        run.err.contains("no input for"),
        "the stop names the gate that ran out of input: {}",
        run.err
    );
    f.cleanup();
}

#[test]
fn a_non_interactive_failure_keeps_the_config_and_leaves_the_containers_up() {
    // Nothing is stopped after a failure, on purpose. A config that names a
    // container which is still up is the state an operator can act on: they can
    // read the logs, and re-running picks up from the config rather than answering
    // the flags again. Taking the container down instead would leave a config that
    // describes a machine that is not there.
    //
    // The config is written before anything runs, so an hour of flags is never
    // thrown away because a sidecar would not boot.
    let f = fixture("flow-noninteractive");
    let runner = StubDocker::ok();
    let run = f.run(broken_managed(), "", false, &runner);

    assert_eq!(run.code, ExitCode::from(1));
    assert!(run.err.contains("clio setup:"), "{}", run.err);
    assert!(
        run.err.contains("your settings were saved"),
        "the message has to say the config is there: {}",
        run.err
    );
    assert!(
        run.err.contains("docker logs"),
        "the message has to say where to look: {}",
        run.err
    );
    assert!(
        f.config.exists(),
        "the flags the operator passed are kept: {}",
        run.err
    );
    assert!(
        !contains_call(&runner.invocations(), "stop")
            && !contains_call(&runner.invocations(), "down"),
        "nothing is taken down after a failure: {:?}",
        runner.invocations()
    );
    f.cleanup();
}

#[test]
fn a_failed_database_test_attempts_no_repair() {
    // There is no repair. The managed database has no password, so an
    // `ALTER ROLE` has nothing to settle and a second test has nothing new to
    // learn: a refusal is a real one and is reported as it is. Pinning the
    // absence matters more than pinning the old rotation did — a repair that
    // came back would rewrite a role on a machine that no longer has passwords.
    let f = fixture("flow-norepair");
    let runner = StubDocker::ok();
    let mut args = broken_managed();
    args.push("--allow-untested".to_owned());
    let run = f.run_with(args, "", false, &runner, &fails_once_database_probe());

    assert_eq!(run.code, ExitCode::from(1), "err: {}", run.err);
    assert!(
        run.err.contains("did not accept the"),
        "the refusal is reported, not repaired: {}",
        run.err
    );
    for call in runner.invocations() {
        assert!(
            !call.iter().any(|arg| arg == "exec" || arg == "psql"),
            "no rotation may be attempted: {call:?}"
        );
    }
    f.cleanup();
}

// # Why there is no test for the recreate path
//
// On master a managed database whose password and server disagree could be
// repaired by typing `recreate`, which authorised `docker compose down -v` and
// deleted every memory in the volume. That path is gone: `compose_ops::recreate_volume`
// is now dead code and nothing calls it, so there is no prompt left to drive and no
// behaviour left to assert. These two tests used to pin it.
//
// They are not rewritten, because a rewritten test would have to assert something
// else and would carry a name describing a prompt that does not exist. The absence
// of the path is itself the thing worth pinning, so what replaces them is one test
// that no code path can reach a volume removal from.

#[test]
fn no_wizard_path_can_remove_a_volume() {
    // A managed run whose container is healthy, and one whose probe fails, are the
    // two shapes that used to differ only in whether the operator typed `recreate`.
    // Neither may reach `down -v` now: the volume is the only copy of the data, and
    // nothing in the wizard is allowed to delete it.
    for (name, runner) in [
        ("flow-volume-ok", StubDocker::ok()),
        ("flow-volume-fail", StubDocker::failing_when_arg("up")),
    ] {
        let f = fixture(name);
        let mut args = broken_managed();
        args.extend(["--allow-untested".to_owned(), "--interactive".to_owned()]);
        let run = f.run(args, "y\n", true, &runner);
        let calls = runner.invocations();
        assert!(
            !calls
                .iter()
                .any(|call| call.contains(&"down".to_owned()) && call.contains(&"-v".to_owned())),
            "{name}: the wizard must never remove a volume: {calls:?}"
        );
        assert!(
            !run.out.contains("Type `recreate`"),
            "{name}: there is no recreate prompt to answer: {}",
            run.out
        );
        f.cleanup();
    }
}

#[test]
fn the_applied_summary_names_the_parts_the_key_and_no_secret() {
    let f = fixture("flow-summary");
    let args = flags(&[
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
        &[
            "--embed",
            "local",
            "--rerank",
            "skip",
            "--extract",
            "skip",
            "--allow-untested",
        ],
    ]);
    let run = f.run_healthy(args, "", false, &StubDocker::ok());

    assert_eq!(run.code, ExitCode::SUCCESS, "err: {}", run.err);
    assert!(
        run.out.contains("Applied. 5 settings written to"),
        "{}",
        run.out
    );
    assert!(
        run.out.contains(
            "Database: Postgres at 127.0.0.1:34310, encrypted and verified when offered, \
                 this machine only"
        ),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains(&format!("Embedding: {LOCAL_EMBED_URL} (this machine only)")),
        "{}",
        run.out
    );
    assert!(!run.out.contains("Reranking:"), "{}", run.out);

    // Asserted on the shape of the line, not on the value after it. The value is
    // whatever the host's key store reports, which is a path on a laptop and an
    // error on a Linux runner with no Secret Service — a comparison would pass here
    // and fail there while saying nothing about the code. The line has to name the
    // store; where the store is, is the store's business.
    assert!(
        run.out.contains("Encryption key: "),
        "the summary must name where the key lives: {}",
        run.out
    );
    assert!(
        !run.out.contains("Encryption key: \n"),
        "the line must not be empty: {}",
        run.out
    );
    // The managed address is printed as host and port only. It carries no
    // password, so there is nothing to leak here, and a redaction mark on it
    // would claim a secret that was never there.
    assert!(
        !run.out.contains("***") && !run.err.contains("***"),
        "a passwordless address is not redacted: {}{}",
        run.out,
        run.err
    );
    f.cleanup();
}

#[test]
fn leaving_at_a_failed_connection_writes_nothing() {
    // The apply gate is a yes-or-no, so there is no word that leaves from it: `No`
    // opens the amend menu and the prompt's own cancel is the only way out, which a
    // reader cannot produce. So the run ends on a usage stop. The config is what the
    // operator would have to clean up by hand, so its absence is the assertion —
    // and it holds on the usage stop as much as it would on a clean exit.
    let f = fixture("flow-quit-after-failure");
    let runner = StubDocker::ok();
    let run = f.run_with(
        broken_sidecar(),
        "",
        true,
        &runner,
        &failing_endpoint_probe(),
    );

    assert_eq!(run.code, ExitCode::from(2), "err: {}", run.err);
    assert!(!f.config.exists(), "leaving must not write: {}", run.err);
    f.cleanup();
}

#[test]
fn agreeing_at_a_failed_connection_saves_the_answers_anyway() {
    // Agreeing to the gate has to mean "save anyway", not "go back": the operator
    // has just read a failure and must not have to retype an hour of answers to get
    // past it. So the config is written whatever the containers do afterwards.
    //
    // The sidecar failing after the write is the case that proves it, because the
    // run ends in a refusal — the answers are still on disk, which is the whole
    // promise. The `--allow-untested` waiver is *not* what this exercises: it waives
    // a probe asked at question time, before the preview, and the warning for that
    // is covered where it is raised. Nothing waives the check that runs after the
    // containers are up, because by then the operator has agreed and a silent pass
    // would be a lie about a service that is not answering.
    let f = fixture("flow-save-anyway");
    let runner = StubDocker::ok();
    // Two questions, because the flags answer everything else: the SQLite file,
    // then the apply gate. A blank at the gate would be `No`, which opens the
    // amend menu rather than agreeing, so the agreement is spelled out as `y`.
    let run = f.run_with(
        broken_sidecar(),
        "/tmp/clio-save-anyway.db\ny\n",
        true,
        &runner,
        &failing_endpoint_probe(),
    );

    assert_eq!(run.code, ExitCode::from(1), "err: {}", run.err);
    assert!(
        run.err.contains("your settings were saved"),
        "the refusal has to say the config is there: {}",
        run.err
    );
    assert!(
        f.config.exists(),
        "save anyway writes what was answered: {}",
        run.err
    );
    f.cleanup();
}
