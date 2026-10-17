// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for process-level contract tests of `clio mcp
//! http` startup: the auto-bind port choice, the explicit-address override, the
//! stderr announcement, stdout purity, and the credential-required auth failure.
//!
//! ## Owns
//! - Spawning the real `clio` binary and asserting its startup streams/ports
//!
//! ## Does not own
//! - In-process bind-helper tests (see `http_bind_tests`)
//! - Transport protocol behavior (see `clio_mcp` conformance tests)
//!
//! ## Boundary
//!
//! Tests here break when the CLI startup contract changes; they never change it.
//!
//! Keep this module focused on the process-level HTTP startup contract.

use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// First and last port of the shared auto-bind range (`34300-34309`).
const FIRST_PORT: u16 = 34300;
const LAST_PORT: u16 = 34309;

fn spawn_http(extra: &[&str]) -> Child {
    let mut args = vec!["mcp", "http"];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_clio"))
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio mcp http")
}

/// Spawn the binary with extra environment entries forced onto the child, and
/// the token-store path override removed so the host cannot hand the child a
/// store the test did not ask for.
fn spawn_http_with_env(extra: &[&str], envs: &[(&str, &str)]) -> Child {
    let mut args = vec!["mcp", "http"];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_clio"))
        .args(&args)
        .env_remove("CLIO_AUTH_TOKEN_FILE")
        .envs(envs.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio mcp http")
}

/// Read the announced listening address from a child's stderr, waiting for the
/// line to appear (the server then blocks serving). An empty string means the
/// line never arrived.
fn announced_addr(child: &mut Child) -> String {
    let stderr = child.stderr.take().expect("stderr piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(addr) = line.strip_prefix("mcp http listening on ") {
                let _ = tx.send(addr.to_owned());
                return;
            }
        }
        let _ = tx.send(String::new());
    });
    rx.recv_timeout(Duration::from_secs(15)).unwrap_or_default()
}

/// Kill the child and return everything it wrote to stdout.
fn kill_and_collect_stdout(mut child: Child) -> Vec<u8> {
    let _ = child.kill();
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut out);
    }
    let _ = child.wait();
    out
}

fn port_of(addr: &str) -> u16 {
    addr.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

#[test]
fn auto_bind_announces_on_stderr_and_keeps_stdout_pure() {
    let mut child = spawn_http(&["--db", "sqlite::memory:", "--insecure-no-auth"]);
    let addr = announced_addr(&mut child);
    assert!(!addr.is_empty(), "no listening line on stderr");
    assert!(addr.starts_with("127.0.0.1:"), "not loopback: {addr}");
    let port = port_of(&addr);
    assert!(
        (FIRST_PORT..=LAST_PORT).contains(&port),
        "auto bind left the range: {addr}"
    );
    let stdout = kill_and_collect_stdout(child);
    assert!(
        stdout.is_empty(),
        "stdout must carry only MCP data, got {:?}",
        String::from_utf8_lossy(&stdout)
    );
}

#[test]
fn explicit_address_is_bound_exactly() {
    // A just-dropped probe port can be taken by a parallel test before the
    // child binds it, so retry the whole probe-and-spawn until it sticks.
    for _ in 0..10 {
        let probe = TcpListener::bind("127.0.0.1:0").expect("probe bind");
        let port = probe.local_addr().expect("probe addr").port();
        drop(probe);
        let want = format!("127.0.0.1:{port}");
        let mut child = spawn_http(&[
            "--bind",
            &want,
            "--db",
            "sqlite::memory:",
            "--insecure-no-auth",
        ]);
        let addr = announced_addr(&mut child);
        let _ = kill_and_collect_stdout(child);
        if addr != want {
            continue;
        }
        assert_eq!(addr, want, "explicit address must win");
        return;
    }
    panic!("no explicit port could be bound exactly in 10 attempts");
}

#[test]
fn two_instances_use_different_ports() {
    let mut a = spawn_http(&["--db", "sqlite::memory:", "--insecure-no-auth"]);
    let addr_a = announced_addr(&mut a);
    let mut b = spawn_http(&["--db", "sqlite::memory:", "--insecure-no-auth"]);
    let addr_b = announced_addr(&mut b);
    assert!(
        !addr_a.is_empty() && !addr_b.is_empty(),
        "both must announce"
    );
    assert_ne!(addr_a, addr_b, "two instances must not share a port");
    for addr in [&addr_a, &addr_b] {
        let port = port_of(addr);
        assert!((FIRST_PORT..=LAST_PORT).contains(&port), "{addr}");
    }
    let _ = kill_and_collect_stdout(a);
    let _ = kill_and_collect_stdout(b);
}

#[test]
fn non_loopback_without_a_credential_fails_closed() {
    // Each case refuses, and each says why. The store load runs before the bind
    // is judged, so an unusable store is named by the store refusal rather than
    // by the bind one — both are refusals, and neither reaches a listener.
    for (extra, why, expected) in [
        (
            Vec::new(),
            "no credential source at all",
            "bind requires an auth token",
        ),
        (
            vec!["--token-file", "/nonexistent/clio-auth-store.json"],
            "a token file that does not exist",
            "does not exist",
        ),
    ] {
        let mut args = vec!["--bind", "0.0.0.0:0", "--db", "sqlite::memory:"];
        args.extend(extra.iter().copied());
        let out = spawn_http(&args).wait_with_output().expect("wait");
        assert!(!out.status.success(), "must fail closed: {why}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(expected),
            "{why}: unexpected stderr: {stderr}"
        );
        assert!(
            !stderr.contains("listening on"),
            "{why}: a rejected bind was announced as listening: {stderr}"
        );
        assert!(
            out.stdout.is_empty(),
            "stdout must stay pure on failure ({why}): {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

/// The legacy environment source is removed: a non-loopback bind with the old
/// variable set on the child, and no token store, is refused rather than served.
/// Only a real process proves this, because the CLI reads its environment
/// directly and no injected-lookup unit test reaches that line.
#[test]
fn the_removed_token_environment_variable_cannot_supply_a_credential() {
    let child = spawn_http_with_env(
        &["--bind", "0.0.0.0:0", "--db", "sqlite::memory:"],
        &[("CLIO_MCP_TOKEN", "a-secret-nobody-reads")],
    );
    let out = child.wait_with_output().expect("wait");
    assert!(
        !out.status.success(),
        "the environment variable must not authorize the bind"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bind requires an auth token"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stderr.contains("listening on"),
        "a rejected bind was announced as listening: {stderr}"
    );
    assert!(
        out.stdout.is_empty(),
        "stdout must stay pure on failure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}
