// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the start-up refusal contract of `clio mcp
//! http --token-file`: every credential store the loader must reject stops the
//! process before it binds, and the help text names the flag and those rules.
//!
//! ## Owns
//! - The absent, malformed, empty, empty-of-records, and group-readable store
//!   refusals
//! - The help-text contract for the flag
//!
//! ## Does not own
//! - The harness (`mcp_http_token_file_support`)
//! - The live per-credential answers (`mcp_http_token_file_binding`)
//! - The loader's own rules (see `clio-mcp`'s `http_auth`)
//!
//! ## Boundary
//!
//! Changes to what a start refuses, or to what the help text promises, should
//! normally require changes here. Changes to how a served request is answered
//! should not.
//!
//! Keep this module focused on the start-up path and nothing that runs after
//! the process has bound a port.

mod common;
#[path = "mcp_http_token_file_support/mod.rs"]
mod mcp_http_token_file_support;

use mcp_http_token_file_support::{Store, run_to_end};
// Only the permissions case reads it, so the import is gated with that test.
#[cfg(unix)]
use std::fs;

#[test]
fn an_absent_token_file_refuses_the_start() {
    let out = run_to_end(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        "/nonexistent/clio-auth-store.json",
    ]);
    assert!(!out.status.success(), "must fail closed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(
        out.stdout.is_empty(),
        "stdout must stay pure: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_malformed_token_file_refuses_the_start() {
    let store = Store::new("malformed");
    store.write_raw("not json at all");
    let out = run_to_end(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        store.path(),
    ]);
    assert!(!out.status.success(), "must fail closed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("malformed"), "{stderr}");
}

#[test]
fn an_empty_file_refuses_the_start() {
    let store = Store::new("emptyfile");
    store.write_raw("");
    let out = run_to_end(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        store.path(),
    ]);
    assert!(!out.status.success(), "must fail closed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("malformed"), "{stderr}");
}

#[test]
fn a_store_with_no_record_refuses_the_start() {
    // It would authorize nobody, so every request would be a 401. Failing at
    // start tells the operator that instead.
    let store = Store::new("norecord");
    store.write_raw("[]");
    let out = run_to_end(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        store.path(),
    ]);
    assert!(!out.status.success(), "must fail closed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("holds no record"), "{stderr}");
    assert!(stderr.contains("auth token add"), "{stderr}");
}

#[test]
#[cfg(unix)]
fn a_group_readable_token_file_refuses_the_start() {
    let store = Store::new("loose");
    store.write_raw("[]");
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o644)).expect("loosen");
    }
    let out = run_to_end(&[
        "mcp",
        "http",
        "--db",
        "sqlite::memory:",
        "--bind",
        "127.0.0.1:0",
        "--token-file",
        store.path(),
    ]);
    assert!(!out.status.success(), "must fail closed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("owner-only"), "{stderr}");
}

#[test]
fn the_help_text_names_the_flag_and_its_refusal_rules() {
    let out = run_to_end(&["mcp", "http", "--help"]);
    assert!(out.status.success(), "help must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--token-file"), "{stdout}");
    assert!(stdout.contains("--insecure-no-auth"), "{stdout}");
    // The refusal rules are named, not implied.
    assert!(stdout.contains("absent"), "{stdout}");
    assert!(stdout.contains("holds no record"), "{stdout}");
}
