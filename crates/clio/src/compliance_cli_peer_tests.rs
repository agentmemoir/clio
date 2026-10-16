// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `compliance inventory` enabled-peers row: the
//! enablement fact the command reads from the deployment's effective
//! configuration and enabled credential records, and the wording the row
//! renders — not applicable for a single-host deployment, enabled without an
//! acknowledgement record for a peer deployment, and the truthfully unverified
//! row when the fact cannot be read.
//!
//! ## Owns
//! - The injected-runner and scratch fixtures these wording tests drive
//! - The assertions over the `enabled_peers` row's source, gap, and open reason
//!
//! ## Does not own
//! - The command under test (`compliance_cli`) or the row's semantics
//!   (see `clio-compliance`)
//! - The rest of the report's shape (`compliance_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use serde_json::{Value, json};

/// An injected env lookup over fixed pairs; every unnamed key reads as absent,
/// so no test reaches the host environment (the `compliance_cli_tests` pattern).
fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
    }
}

/// A unique scratch directory, removed on drop (the `compliance_cli_tests`
/// scratch pattern).
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clio-compliance-peers-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `inventory` against one injected env and a missing store, and return the
/// report's `enabled_peers` row.
fn peers_row(env: &[(&str, &str)], scratch: &Scratch) -> Value {
    let lookup = env_from(env);
    let store = scratch.path().join("store.db");
    let store_arg = store.display().to_string();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::Cursor::new(Vec::new());
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &lookup,
        };
        run(
            &mut io,
            vec![
                "inventory".to_owned(),
                "--db".to_owned(),
                store_arg,
                "-o".to_owned(),
                "json".to_owned(),
            ],
            Ok(scratch.path().to_path_buf()),
        )
    };
    let out = String::from_utf8(out).expect("utf8 out");
    let err = String::from_utf8(err).expect("utf8 err");
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json report");
    payload["rows"]
        .as_array()
        .expect("rows array")
        .iter()
        .find(|row| row["family"] == json!("enabled_peers"))
        .cloned()
        .expect("the report names the enabled_peers family")
}

/// One valid credential record naming `label` and reaching the demo bank.
fn credential_record(label: &str) -> clio_config::auth_tokens::AuthTokenRecord {
    clio_config::auth_tokens::AuthTokenRecord {
        label: label.to_owned(),
        secret_sha256: "0123456789abcdef".repeat(4),
        actor: clio_config::context::Actor::Agent,
        banks: vec!["demo".to_owned()],
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

/// A deployment overlay whose sync is in play at `scratch`.
fn sync_enabled_overlay(scratch: &Scratch) -> String {
    let path = scratch.path().join("deployment.json");
    std::fs::write(&path, r#"{"sync":{"omit":false}}"#).expect("write the deployment overlay");
    path.display().to_string()
}

/// A single-host (sync omitted) deployment records the peer family as not
/// applicable: no gap, no open reason, and a source that says so.
#[test]
fn the_peers_row_reports_not_applicable_for_a_single_host_deployment() {
    let scratch = Scratch::new("single-host");
    let row = peers_row(&[], &scratch);
    assert_eq!(
        row["source"],
        json!("peer enablement: not applicable (no enabled peers)"),
        "{row}"
    );
    assert_eq!(row["open"], json!(false), "{row}");
    assert!(row["gap"].is_null(), "{row}");
    assert!(row["open_reason"].is_null(), "{row}");
}

/// A deployment with sync in play and one credential record reports the
/// enabled fact without claiming an acknowledgement this command cannot read.
#[test]
fn the_peers_row_reports_enabled_without_an_acknowledgement_record() {
    let scratch = Scratch::new("enabled");
    let deployment = sync_enabled_overlay(&scratch);
    let tokens = scratch.path().join("auth_tokens.json");
    clio_config::auth_tokens::save_auth_tokens(&tokens, &[credential_record("peer-a")])
        .expect("save the credential store");
    let token_arg = tokens.display().to_string();
    let env = [
        ("CLIO_DEPLOYMENT_CONFIG", deployment.as_str()),
        ("CLIO_AUTH_TOKEN_FILE", token_arg.as_str()),
    ];
    let row = peers_row(&env, &scratch);
    assert_eq!(
        row["source"],
        json!("peer enablement: enabled (no acknowledgement record supplied)"),
        "{row}"
    );
    assert_eq!(row["open"], json!(true), "{row}");
    assert_eq!(
        row["open_reason"],
        json!(
            "no acknowledgement record exists to verify a peer's receipt; \
             peers are enabled and no acknowledgement record exists to verify them"
        ),
        "{row}"
    );
}

/// A deployment with sync in play whose credential store cannot be read keeps
/// the truthful not-supplied fact: the row stays open and says it was not
/// verified, instead of claiming peers or a single host.
#[test]
fn the_peers_row_stays_unverified_when_the_credential_store_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("unreadable");
    let deployment = sync_enabled_overlay(&scratch);
    let tokens = scratch.path().join("auth_tokens.json");
    std::fs::write(&tokens, "not a token store").expect("write the broken store");
    std::fs::set_permissions(&tokens, std::fs::Permissions::from_mode(0o644))
        .expect("widen the token store");
    let token_arg = tokens.display().to_string();
    let env = [
        ("CLIO_DEPLOYMENT_CONFIG", deployment.as_str()),
        ("CLIO_AUTH_TOKEN_FILE", token_arg.as_str()),
    ];
    let row = peers_row(&env, &scratch);
    assert_eq!(
        row["source"],
        json!("peer enablement and acknowledgements (not stored)"),
        "{row}"
    );
    assert_eq!(row["open"], json!(true), "{row}");
    assert_eq!(
        row["open_reason"],
        json!(
            "no acknowledgement record exists to verify a peer's receipt; \
             peers could not be verified: no enablement fact was supplied"
        ),
        "{row}"
    );
}
