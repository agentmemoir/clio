// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the CLI state a `clio config` command saves is read
//! by a later, separate `clio` process: the profile `config use` activates, the
//! profile-scoped value `config set --scope profile` writes, the refusal of a
//! session-scoped set, and the fact that a boot restore of that state is quiet
//! while an explicit activation logs.
//!
//! ## Owns
//! - End-to-end `clio config` assertions over spawned processes
//!
//! ## Does not own
//! - In-process CLI state tests (see `clio-config`'s `cli_state_tests`)
//! - Config merge order and persistence (`clio-config`)
//!
//! ## Boundary
//!
//! Tests here break when the saved-state contract between two real `clio`
//! processes changes; they never change it.
//!
//! Keep this module focused on the cross-process config contract.

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so two scenarios in one binary
/// never share a config directory.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A config directory of this scenario's own, under the system temp dir.
struct ConfigDir(PathBuf);

impl ConfigDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "clio-cli-state-{tag}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create config dir");
        Self(path)
    }
}

impl Drop for ConfigDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The `clio` binary with a cleared environment and the scenario's config
/// directory as its only XDG root, so no host setting reaches it.
fn clio(dir: &ConfigDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    // Forward the coverage profile pattern so instrumented children write into
    // the gitignored target directory instead of the source tree.
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd.env("XDG_CONFIG_HOME", &dir.0);
    cmd.env("HOME", &dir.0);
    cmd
}

/// One command's full output, with piped streams.
fn run(dir: &ConfigDir, args: &[&str]) -> Output {
    clio(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

/// Parse one command's stdout as JSON, with the raw streams on failure.
fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Assert a command exited 0, reporting its streams when it did not.
fn succeeded(output: &Output, what: &str) -> String {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{what} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn a_profile_used_in_one_process_is_active_and_readable_in_later_processes() {
    let dir = ConfigDir::new("state");

    let used = run(&dir, &["config", "use", "coding_local", "--output", "json"]);
    let payload = json_stdout(&{
        succeeded(&used, "config use");
        used
    });
    assert_eq!(payload["name"], "coding_local");
    assert!(
        payload["precedence_note"]
            .as_str()
            .is_some_and(|note| note.contains("CLIO_LOG_LEVEL") && note.contains("CLIO_BACKEND")),
        "{payload}"
    );

    let profiles = run(&dir, &["config", "profiles", "--output", "json"]);
    let listed = json_stdout(&{
        succeeded(&profiles, "config profiles");
        profiles
    });
    assert_eq!(
        listed["active"], "coding_local",
        "a later process must report the profile an earlier one activated: {listed}"
    );

    let set = run(
        &dir,
        &[
            "config",
            "set",
            "log.level",
            "debug",
            "--scope",
            "profile",
            "--output",
            "json",
        ],
    );
    let applied = json_stdout(&{
        succeeded(&set, "config set --scope profile");
        set
    });
    assert_eq!(applied["value"], "debug", "{applied}");

    let get = run(&dir, &["config", "get", "log.level", "--output", "json"]);
    let read = json_stdout(&{
        succeeded(&get, "config get log.level");
        get
    });
    assert_eq!(
        read["value"], "debug",
        "a later process must read the profile-scoped value: {read}"
    );
}

#[test]
fn a_session_scoped_set_exits_non_zero_in_a_one_command_process() {
    let dir = ConfigDir::new("session");
    let refused = run(
        &dir,
        &[
            "config",
            "set",
            "log.level",
            "debug",
            "--scope",
            "session",
            "--output",
            "json",
        ],
    );
    assert_ne!(
        refused.status.code(),
        Some(0),
        "the CLI must refuse a session-scoped write; stdout: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.to_lowercase().contains("session"),
        "the refusal must name the scope: {stderr}"
    );
}

#[test]
fn a_boot_restore_of_the_saved_profile_logs_no_activation_while_use_does() {
    let dir = ConfigDir::new("log");
    succeeded(
        &run(&dir, &["config", "use", "coding_local", "--output", "text"]),
        "config use",
    );

    let quiet = run(&dir, &["config", "profiles", "--output", "text"]);
    succeeded(&quiet, "config profiles");
    let stderr = String::from_utf8_lossy(&quiet.stderr);
    assert!(
        !stderr.contains("config_profile_apply"),
        "a boot restore must not log the activation the user never ran: {stderr}"
    );

    let loud = run(&dir, &["config", "use", "coding_local", "--output", "text"]);
    succeeded(&loud, "config use coding_local");
    let stderr = String::from_utf8_lossy(&loud.stderr);
    assert!(
        stderr.contains("config_profile_apply"),
        "an explicit activation still logs: {stderr}"
    );
}
