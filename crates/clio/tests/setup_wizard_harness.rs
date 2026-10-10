// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for end-to-end tests of the real `clio` binary: a
//! run with piped stdin and no TTY, in a temporary HOME, against a config path
//! the test owns.
//!
//! ## Owns
//! - Process-level scenarios for the `clio setup` command line
//! - The promise that a full run applies its config and says so
//! - The promise that no secret reaches the captured output
//!
//! ## Does not own
//! - In-process wizard tests (see the crate internals)
//! - The `clio` binary under test
//! - What a failure leaves behind: every scenario here asserts that too
//!
//! ## Boundary
//!
//! Changes to the setup-to-binary contract should normally update tests here.
//! Changes to in-process setup internals should not require changes here.
//!
//! Keep this module focused on the setup end-to-end harness.
//!
//! # Why the run is non-interactive
//!
//! stdin is a closed pipe, so the wizard auto-detects no terminal and never
//! prompts. The apply path is therefore reached without a TTY, and a run that
//! needs an answer has to fail fast rather than block.
//!
//! # Why the failures are fast
//!
//! The typed route points at the suite's own Postgres, on the port
//! [`test_pg_port`] reads from the environment, with credentials it does not
//! accept. A server answers, so the failure is a refusal rather than a timeout,
//! and the same three-attempt path is exercised without a fifteen-second wait per
//! attempt. A hostname that does not resolve is retried for fifteen seconds per
//! attempt, which turns a scenario into a minute-long wait — and a port with
//! nothing listening on it is retried just as patiently, which is why the address
//! has to be a real server's rather than a number that happens to be the right one
//! on one machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn clio_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clio"))
}

fn temp_dir(tag: &str) -> PathBuf {
    // The counter is what makes this distinct: the process id is constant
    // inside one test binary, so clock-plus-pid alone repeats whenever two
    // calls land in the same tick, and two scenarios then share one
    // directory and read each other's database.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "am-e2e-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    // A Clio encryption key file, seeded here because the deployment config
    // seals the API keys the wizard writes. The file is authoritative once it
    // exists, so the `clio` process under test never asks the operating system's
    // secret store to create a key — which on a developer machine would mean
    // writing a real entry into their login Keychain. This is also the path CI
    // takes, so the fallback is exercised rather than assumed.
    std::fs::write(dir.join("clio.key"), [0x6b_u8; 32]).expect("seed the test key file");
    dir
}

struct Outcome {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Outcome {
    /// Everything the process printed, on either stream.
    fn captured(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Run `clio` with a temp HOME/XDG and no `CLIO_DEPLOYMENT_CONFIG`, feeding `stdin`
/// through a pipe (the no-TTY path).
///
/// The database and embedding variables are removed rather than inherited. A
/// `CLIO_DATABASE_URL` exported in the developer's shell is a real way to point
/// Clio at a database and the wizard honours it, so a scenario that inherits one
/// is asserting a fact about the machine instead of about the command. One
/// scenario below sets it on purpose, to prove the honouring.
fn run_clio(dir: &Path, args: &[&str]) -> Outcome {
    use std::io::Write;
    let mut child = clio_bin()
        .args(args)
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("HOME", dir)
        .env("CLIO_KEY_FILE", dir.join("clio.key"))
        .env("CLIO_KEY_FILE_ONLY", "1")
        .env_remove("CLIO_DEPLOYMENT_CONFIG")
        .env_remove("CLIO_EMBED_URL")
        .env_remove("CLIO_EMBED_MODEL")
        .env_remove("CLIO_DATABASE_URL")
        .env_remove("CLIO_DB")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"")
        .expect("close stdin");
    let out = child.wait_with_output().expect("wait");
    Outcome {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A fully flagged run that starts no container at all.
///
/// The embedding endpoint is a loopback port nothing listens on, so its probe
/// refuses at once and `--allow-untested` carries the run to the applied
/// summary. That keeps this harness independent of Docker: a temp HOME makes the
/// `docker` CLI lose the developer's context and daemon socket, so a scenario
/// that started a sidecar would fail for a reason that has nothing to do with
/// setup.
fn sqlite_run(config: &Path) -> Vec<String> {
    [
        "setup",
        "--database",
        "sqlite",
        "--embed",
        "online",
        "--embed-url",
        "https://127.0.0.1:1",
        "--embed-model",
        "m",
        "--embed-key",
        "sk-embed-secret-1234567890",
        "--rerank",
        "skip",
        "--extract",
        "skip",
        "--allow-untested",
        "--config",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain(std::iter::once(config.to_string_lossy().into_owned()))
    .collect()
}

/// The port the suite's own Postgres is published on, as this machine publishes it.
///
/// Read from the test process's `CLIO_DATABASE_URL`, which is the one variable
/// both places set: the workflow points it at the Actions service on `5432` and
/// the Makefile points it at the Compose stack on `34310`. Both publish on
/// loopback, so the port is the only part of the address that varies.
///
/// A scenario that dials a hardcoded port is asserting a fact about the machine
/// rather than about the wizard, and it fails in a way that reads like the wizard
/// is broken: a closed port answers `Connection refused`, which is one word away
/// from a server refusing the credentials, so the run spends its whole retry
/// budget and then fails on a message about authentication that never happened.
///
/// It is read here and never handed to the child. [`run_clio`] still removes
/// `CLIO_DATABASE_URL` from the environment, so a scenario that does not set one
/// on purpose keeps proving that the wizard does not invent an address.
fn test_pg_port() -> String {
    const COMPOSE_PG_PORT: &str = "34310";
    let Ok(url) = std::env::var("CLIO_DATABASE_URL") else {
        return COMPOSE_PG_PORT.to_owned();
    };
    clio_store::parse_connection(&url)
        .ok()
        .and_then(|parsed| parsed.ports.first().copied())
        .map_or_else(|| COMPOSE_PG_PORT.to_owned(), |port| port.to_string())
}

/// A fully flagged typed run the local database refuses.
///
/// The refusal is a database that does not exist, not a wrong password. The
/// managed Postgres uses trust authentication and has no password, so it can no
/// longer reject a credential — a run aimed at a wrong password would connect
/// and succeed, which is the opposite of what these runs are for. A database
/// name the server does not have is refused the same way, immediately, and with
/// a reason the server names.
///
/// The password is still in the address on purpose: the point of the run is that
/// an address carrying a secret can be refused and reported without the secret
/// reaching any output.
fn typed_run(config: &Path) -> Vec<String> {
    let port = test_pg_port();
    [
        "setup",
        "--database",
        "existing",
        "--database-connect",
        "typed",
        "--database-host",
        "127.0.0.1",
        "--database-port",
        port.as_str(),
        "--database-name",
        "no_such_database_for_this_test",
        "--database-user",
        "clio",
        "--database-password",
        "sk-db-secret-1234567890",
        "--database-ssl",
        "prefer",
        "--embed",
        "online",
        "--embed-url",
        "https://embed.example.test",
        "--embed-model",
        "text-embedding-3-small",
        "--embed-key",
        "sk-embed-secret-1234567890",
        "--rerank",
        "skip",
        "--extract",
        "skip",
        "--config",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain(std::iter::once(config.to_string_lossy().into_owned()))
    .collect()
}

fn as_args(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

#[test]
fn a_full_run_applies_the_config_and_reports_it() {
    let dir = temp_dir("apply");
    let config = dir.join("xdg").join("clio").join("deployment.json");
    let run = run_clio(&dir, &as_args(&sqlite_run(&config)));
    assert!(run.ok, "setup failed: {}", run.stderr);
    assert!(run.stdout.contains("Clio setup"), "{}", run.stdout);
    assert!(run.stdout.contains("Database backend"), "{}", run.stdout);
    assert!(
        run.stdout.contains("Applied. 7 settings written to"),
        "{}",
        run.stdout
    );
    // The preview is drawn before the write, which is what makes it a preview.
    let preview_at = run.stdout.find("Will back up").expect("the preview");
    let applied_at = run.stdout.find("Applied.").expect("the summary");
    assert!(preview_at < applied_at, "{}", run.stdout);
    let saved = std::fs::read_to_string(&config).expect("the config was written");
    assert!(saved.contains("\"backend\""), "{saved}");
    assert!(saved.contains("store.database_url"), "{saved}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_preview_names_what_a_run_would_do_and_what_it_did_not_test() {
    let dir = temp_dir("preview-lines");
    let config = dir.join("xdg").join("clio").join("deployment.json");
    let run = run_clio(&dir, &as_args(&sqlite_run(&config)));
    assert!(run.ok, "setup failed: {}", run.stderr);
    for line in ["Will start", "Will remove", "Will back up", "Tested"] {
        assert!(run.stdout.contains(line), "no {line:?} in:\n{}", run.stdout);
    }
    // A SQLite install has no connection to test, and the line says so rather
    // than pretending a file was opened. That is the deterministic half of the
    // "untested is visibly different" rule; the sidecar's line depends on
    // whether the Compose stack is running.
    assert!(
        run.stdout
            .contains("not tested (no connection was tested on this run)"),
        "{}",
        run.stdout
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_secret_in_captured_output() {
    let dir = temp_dir("secrets");
    let config = dir.join("xdg").join("clio").join("deployment.json");

    // A run that succeeds: the answers are carried into the preview and the
    // applied summary, and neither may echo a secret.
    let run = run_clio(&dir, &as_args(&sqlite_run(&config)));
    let captured = run.captured();
    assert!(!captured.contains("sk-db-secret"), "{captured}");
    assert!(!captured.contains("sk-embed-secret"), "{captured}");

    // A run that fails: the probe is handed the address, so the most useful
    // thing it can say would be the address, and the address carries the
    // password. The failure is printed by the probe line, the blame line, and
    // the final refusal, so this is where a leak would show.
    let failed = run_clio(&dir, &as_args(&typed_run(&config)));
    let captured = failed.captured();
    assert!(!captured.contains("sk-db-secret"), "{captured}");
    assert!(!captured.contains("sk-embed-secret"), "{captured}");
    // The refusal is still actionable: it says which attempt failed and why.
    assert!(
        captured.contains("Testing the connection ... failed"),
        "{captured}"
    );
    // The refusal carries the server's own reason rather than a bare failure.
    //
    // Which reason depends on the machine, exactly as the port does. CI's
    // Postgres uses scram with a password, so the wrong password is refused
    // first and the server names the user; a local managed database uses trust
    // and ignores the password, so the refusal is the database that does not
    // exist. Both are the server speaking, which is the property under test, so
    // the assertion is that the server's own words came through — not which
    // words they were. `test_pg_port` above has the same reason for reading the
    // port instead of hardcoding it.
    assert!(!captured.contains("db error"), "{captured}");
    assert!(
        captured.contains("clio") || captured.contains("no_such_database_for_this_test"),
        "the server's own reason must reach the operator: {captured}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_connection_reports_the_failure_and_writes_nothing() {
    let dir = temp_dir("refused");
    let config = dir.join("xdg").join("clio").join("deployment.json");
    let run = run_clio(&dir, &as_args(&typed_run(&config)));
    assert!(!run.ok, "a refused connection must not report success");
    assert!(run.stderr.contains("nothing was written"), "{}", run.stderr);
    assert!(run.stderr.contains("3 times"), "{}", run.stderr);
    assert!(!config.exists(), "{}", config.display());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_removed_install_type_is_refused_with_the_replacement_named() {
    // The old flag is the one a returning operator types first, so the refusal
    // has to say what to use instead rather than only what is wrong.
    let dir = temp_dir("type-gone");
    let out = run_clio(&dir, &["setup", "--type", "zero-dependency"]);
    assert!(!out.ok, "expected a usage failure");
    assert!(out.stderr.contains("--type was removed"), "{}", out.stderr);
    assert!(out.stderr.contains("--database"), "{}", out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_flags_fail_fast_without_a_tty() {
    let dir = temp_dir("no-tty");
    let out = run_clio(&dir, &["setup", "--non-interactive"]);
    assert!(!out.ok, "expected a usage failure");
    // No flag named a service, so the run must say so rather than start a
    // container. It must also not have blocked on the inherited terminal.
    assert!(!out.stderr.is_empty(), "a refusal must say something");
    assert!(
        !dir.join("xdg")
            .join("clio")
            .join("deployment.json")
            .exists()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_database_in_the_environment_is_offered_as_the_address_in_use() {
    // A `CLIO_DATABASE_URL` in the shell is a real way to point Clio at a
    // database, so the wizard offers it rather than ignoring it. The route below
    // names the backend and the hand-over but none of the address fields, so the
    // fields have to come from the environment: if the wizard ignored it, the run
    // would stop at the first missing field instead of attempting a connection.
    let dir = temp_dir("env-db");
    let config = dir.join("xdg").join("clio").join("deployment.json");
    let env_url = format!(
        "postgres://envuser:envpass@127.0.0.1:{}/envdb",
        test_pg_port()
    );
    let args = [
        "setup",
        "--database",
        "existing",
        "--database-connect",
        "typed",
        "--database-password",
        "typed-secret",
        "--database-ssl",
        "prefer",
        "--embed",
        "local",
        "--rerank",
        "skip",
        "--extract",
        "skip",
        "--config",
        &config.to_string_lossy(),
    ];
    let mut child = clio_bin()
        .args(args)
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("HOME", &dir)
        .env("CLIO_KEY_FILE", dir.join("clio.key"))
        .env("CLIO_KEY_FILE_ONLY", "1")
        .env("CLIO_DATABASE_URL", &env_url)
        .env_remove("CLIO_DEPLOYMENT_CONFIG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(b"")
            .expect("close stdin");
    }
    let out = child.wait_with_output().expect("wait");
    let captured = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The user from the environment is named by the server's refusal, which
    // proves the run adopted the address and got as far as attempting a
    // connection rather than stopping at a missing flag. Neither password may
    // appear: the one in the environment, and the one just typed.
    assert!(captured.contains("envuser"), "{captured}");
    assert!(!captured.contains("envpass"), "{captured}");
    assert!(!captured.contains("typed-secret"), "{captured}");
    let _ = std::fs::remove_dir_all(&dir);
}
