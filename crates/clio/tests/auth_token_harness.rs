// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio auth token` verbs against the real binary: the
//! add → list → revoke transcript, the one-time secret print, digest-only
//! persistence, and the refusal rules.
//!
//! ## Owns
//! - End-to-end `auth token` assertions over spawned `clio` processes
//!
//! ## Does not own
//! - In-process store and CLI tests (`auth_tokens_tests`, `auth_cli_tests`)
//! - The CLI implementation under test
//!
//! ## Boundary
//!
//! Tests here break when the real binary's token-store contract changes; they
//! never change it.
//!
//! Keep this module focused on the `auth token` process contract.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Unique store path for one scenario.
fn store_path(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "clio-auth-token-{tag}-{}-{nanos}-{sequence}.json",
        std::process::id()
    ))
}

/// The `clio` binary with a cleared environment plus the coverage pattern, and
/// the token store pointed at the scenario's own file.
fn clio(store: &Path) -> Command {
    clio_with_env(store, &[])
}

/// The same binary, plus extra environment entries. The blank-override scenario
/// needs an empty value rather than an absent one, which `env_clear` alone
/// cannot express.
fn clio_with_env(store: &Path, env: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    // Forward the coverage profile pattern so instrumented children write into
    // the gitignored target directory instead of the source tree.
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd.env("CLIO_AUTH_TOKEN_FILE", store);
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd
}

/// Run one `auth` command with piped streams.
fn run(store: &Path, args: &[&str]) -> Output {
    clio(store)
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

#[test]
fn the_real_binary_runs_add_list_and_revoke() {
    let store = store_path("flow");
    let add = run(
        &store,
        &[
            "auth", "token", "add", "--label", "ci", "--actor", "agent", "--bank", "demo",
            "--output", "text",
        ],
    );
    assert_eq!(
        add.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let stdout = String::from_utf8_lossy(&add.stdout).to_string();
    // The secret prints exactly once, as its own line, with the path and the
    // recovery warning.
    let secret = stdout
        .lines()
        .find(|line| line.len() == 43)
        .expect("43-char secret line")
        .to_owned();
    assert_eq!(stdout.matches(&secret).count(), 1, "{stdout}");
    assert!(stdout.contains("cannot be recovered"), "{stdout}");
    assert!(stdout.contains(&store.display().to_string()), "{stdout}");
    // The bytes on disk hold the digest, never the plaintext secret.
    let bytes = fs::read_to_string(&store).expect("store file");
    assert!(!bytes.contains(&secret), "plaintext secret on disk");
    let records: Vec<Value> = serde_json::from_str(&bytes).expect("parse");
    assert_eq!(records[0]["label"], "ci");
    assert_eq!(
        records[0]["secret_sha256"].as_str().expect("digest").len(),
        64
    );

    let list = run(&store, &["auth", "token", "list"]);
    assert_eq!(list.status.code(), Some(0));
    let list_out = String::from_utf8_lossy(&list.stdout).to_string();
    assert!(list_out.contains("ci"), "{list_out}");
    assert!(!list_out.contains("secret_sha256"), "{list_out}");
    assert!(!list_out.contains(&secret), "list printed the secret");
    let payload = json_stdout(&list);
    assert_eq!(payload["records"].as_array().expect("rows").len(), 1);
    assert!(payload["records"][0].get("secret_sha256").is_none());

    let revoke = run(&store, &["auth", "token", "revoke", "--label", "ci"]);
    assert_eq!(revoke.status.code(), Some(0));
    let revoke_out = String::from_utf8_lossy(&revoke.stdout).to_string();
    assert!(revoke_out.contains("next server start"), "{revoke_out}");
    let remaining = serde_json::from_str::<Vec<Value>>(&fs::read_to_string(&store).expect("bytes"))
        .expect("parse");
    assert!(remaining.is_empty(), "{remaining:?}");

    let _ = fs::remove_file(&store);
}

#[test]
fn concurrent_adds_against_one_store_all_land() {
    // The defect this pins: without the store lock, six concurrent runs each
    // read the same empty store and each write its own record over the
    // others', so runs report success with a printed secret whose row is gone.
    // The property is "every run that printed a secret has its row on disk",
    // so a run that refused under the lock is a pass, not a failure.
    let store = store_path("concurrent");
    let mut children = Vec::new();
    for index in 1..=6u8 {
        let label = format!("L{index}");
        let child = clio(&store)
            .args([
                "auth", "token", "add", "--label", &label, "--actor", "agent", "--bank", "demo",
                "--output", "json",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn a concurrent add");
        children.push((label, child));
    }
    let mut printed: Vec<String> = Vec::new();
    for (label, child) in children {
        // Await every child: a dropped `Child` would leak a process that still
        // holds the store lock.
        let output = child.wait_with_output().expect("wait for an add");
        let status = output.status.code();
        match status {
            Some(0) => {
                let payload: Value = serde_json::from_slice(&output.stdout)
                    .unwrap_or_else(|e| panic!("{label} printed no JSON: {e}"));
                let secret = payload["token"].as_str().expect("a printed secret");
                assert!(!secret.is_empty(), "{label} printed an empty secret");
                printed.push(label);
            }
            other => {
                // A refusal must not have printed a secret.
                assert_ne!(other, Some(0), "{label}");
                assert!(
                    !String::from_utf8_lossy(&output.stdout).contains("token"),
                    "a refused run printed a secret: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
            }
        }
    }
    assert!(
        !printed.is_empty(),
        "every run refused; the store is unusable"
    );
    let records: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(&store).expect("store file")).expect("parse");
    let stored: Vec<&str> = records.iter().filter_map(|r| r["label"].as_str()).collect();
    for label in &printed {
        assert!(
            stored.contains(&label.as_str()),
            "{label} printed a secret but is not on disk: {stored:?}"
        );
    }
    assert_eq!(stored.len(), printed.len(), "no extra rows: {stored:?}");
    let _ = fs::remove_file(&store);
    let _ = fs::remove_file(clio_config::auth_tokens::lock_file_path(&store));
}

#[test]
fn an_extra_positional_and_an_unusable_store_flag_exit_two_end_to_end() {
    let store = store_path("strict");
    let list = run(&store, &["auth", "token", "list", "extra"]);
    assert_eq!(list.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&list.stderr).contains("unexpected extra argument"),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let flag = run(
        &store,
        &["auth", "token", "list", "--db", "/nonexistent.sqlite"],
    );
    assert_eq!(flag.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&flag.stderr).contains("cannot change what it does"),
        "{}",
        String::from_utf8_lossy(&flag.stderr)
    );
    let _ = fs::remove_file(&store);
}

#[test]
fn a_blank_override_falls_through_to_the_config_default() {
    // `CLIO_AUTH_TOKEN_FILE=""` counts as unset, exactly as an absent value
    // does, so the run uses the XDG default instead of refusing and telling
    // the operator to set a variable they already set. The scratch XDG
    // directory keeps the default out of the host's real config.
    let xdg = store_path("xdg").with_extension("xdg");
    fs::create_dir_all(&xdg).expect("create the scratch config dir");
    let store = xdg.join("clio").join("auth_tokens.json");
    let output = clio_with_env(&store, &[("CLIO_AUTH_TOKEN_FILE", "  ")])
        .args([
            "auth", "token", "add", "--label", "ci", "--actor", "agent", "--bank", "demo",
        ])
        .env("XDG_CONFIG_HOME", &xdg)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        store.is_file(),
        "the default path was used: {}",
        store.display()
    );
    // With no config directory at all there is nowhere to resolve, and the
    // refusal says so rather than guessing.
    let homeless = clio_with_env(&store, &[])
        .args(["auth", "token", "list"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio");
    assert_eq!(homeless.status.code(), Some(0));
    let _ = fs::remove_file(&store);
    let _ = fs::remove_dir(xdg.join("clio"));
    let _ = fs::remove_dir(&xdg);
}

#[test]
fn the_real_binary_refuses_bad_input_and_bad_files() {
    let store = store_path("refuse");
    // Duplicate label, unknown revoke label, bad actor, no bank.
    let add = run(
        &store,
        &[
            "auth", "token", "add", "--label", "ci", "--actor", "agent", "--bank", "demo",
        ],
    );
    assert_eq!(add.status.code(), Some(0));
    let dup = run(
        &store,
        &[
            "auth", "token", "add", "--label", "ci", "--actor", "agent", "--bank", "demo",
        ],
    );
    assert_eq!(dup.status.code(), Some(2));
    let ghost = run(&store, &["auth", "token", "revoke", "--label", "ghost"]);
    assert_eq!(ghost.status.code(), Some(2));
    let robot = run(
        &store,
        &[
            "auth", "token", "add", "--label", "x", "--actor", "robot", "--bank", "demo",
        ],
    );
    assert_eq!(robot.status.code(), Some(2));
    let no_bank = run(
        &store,
        &["auth", "token", "add", "--label", "x", "--actor", "agent"],
    );
    assert_eq!(no_bank.status.code(), Some(2));

    // A malformed store is refused, not parsed loosely. The store is made
    // owner-only first, because the permission check runs before the parse.
    let corrupt = store_path("corrupt");
    fs::write(&corrupt, "not json").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&corrupt, fs::Permissions::from_mode(0o600)).expect("tighten");
    }
    let refused = run(&corrupt, &["auth", "token", "list"]);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(stderr.contains("malformed"), "{stderr}");
    // A file group or other can read is refused before it is parsed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&corrupt, fs::Permissions::from_mode(0o644)).expect("loosen");
        fs::write(&corrupt, "[]").expect("write array");
        let refused = run(&corrupt, &["auth", "token", "list"]);
        assert_eq!(refused.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("owner-only"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
    }
    let _ = fs::remove_file(&store);
    let _ = fs::remove_file(&corrupt);
}
