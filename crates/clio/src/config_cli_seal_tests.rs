// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests what `clio config set` leaves on disk when the value it
//! applies carries a secret of its own rather than sitting at a path with a
//! secret name: a deployment-scoped endpoint URL with userinfo credentials.
//!
//! ## Owns
//! - The deployment-scoped `config set` write of a URL whose userinfo holds a
//!   password: the file holds a sealed envelope, never the password, and the
//!   reported value is still masked
//!
//! ## Does not own
//! - The command implementation or its views (`config_cli`)
//! - The sealing rule itself and the envelope format (`clio-config`)
//! - The general config verb matrix and its normal-path tests (`config_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{json_of, run, runtime_with_deployment_file};
use super::*;
use serde_json::json;

/// A URL whose userinfo carries a password, which is the value the report used.
const USERINFO_URL: &str = "https://user:DUMMY_PASSWORD@host";

#[test]
fn config_set_scope_deployment_seals_a_url_with_userinfo_and_reports_it_masked() {
    let (mut rt, dir) = runtime_with_deployment_file();
    let path = dir.join("deployment.json");

    let (code, out, err) = run(
        &["set", "extract.url", USERINFO_URL, "--scope", "deployment"],
        &mut rt,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["value"], json!("[REDACTED]"), "{out}");
    assert!(!out.contains("DUMMY_PASSWORD"), "{out}");

    let text = std::fs::read_to_string(&path).expect("the deployment set wrote the file");
    assert!(
        !text.contains("DUMMY_PASSWORD"),
        "the password reached the file: {text}"
    );
    let stored = serde_json::from_str::<Value>(&text).expect("the file is a flat JSON object");
    let Some(Value::String(envelope)) = stored.get("extract.url") else {
        panic!("the stored value is a string: {text}");
    };
    assert!(
        clio_config::secret_seal::is_sealed(envelope),
        "the stored value is an envelope: {text}"
    );

    let (_, applied, err) = run(
        &["set", "extract.url", USERINFO_URL, "--scope", "deployment"],
        &mut rt,
        true,
    );
    assert!(!applied.contains("DUMMY_PASSWORD"), "{applied}{err}");
    assert!(
        applied.contains("config set extract.url (scope deployment) = [REDACTED]"),
        "{applied}"
    );

    // The read that follows opens the file this set wrote, so the value the
    // command hands back is the masked one.
    let (_, read_back, err) = run(&["get", "extract.url"], &mut rt, true);
    assert!(!read_back.contains("DUMMY_PASSWORD"), "{read_back}{err}");
    assert!(
        read_back.contains("config extract.url = [REDACTED]"),
        "{read_back}"
    );
}
