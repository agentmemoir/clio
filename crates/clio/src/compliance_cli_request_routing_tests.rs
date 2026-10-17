// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the request and peer-request routing seam: the verb list
//! the `compliance` dispatcher routes to `compliance_request_cli` and the
//! registry's `request-*` and `peer-*` subset stay equal.
//!
//! ## Owns
//! - The parity assertion between the routed request/peer verb list and the
//!   registry subset derived from `compliance_subcommands()`
//!
//! ## Does not own
//! - The dispatch itself (`compliance_cli`) or the registry lookups
//!   (`cli_command_registry`)
//! - The dispatcher-completeness scan (`cli_help_dispatch_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the request and peer-request routing parity.

use super::*;
use crate::cli_error::EXIT_USAGE;

/// The request and peer-request verbs the `compliance` dispatcher routes to
/// `compliance_request_cli`: the registry's `request-*` and `peer-*` subset.
/// The parity test pins this list against that subset, so the routed set cannot
/// drift from the registry.
const REQUEST_SUBCOMMANDS: &[&str] = &[
    "request-start",
    "request-status",
    "request-retry",
    "request-copy",
    "request-exception",
    "request-events",
    "request-close",
    "peer-status",
    "peer-retry",
    "peer-disposition",
];

/// The routed request and peer-request verb list equals the registry's
/// `request-*` and `peer-*` subset, so a verb added to the registry cannot go
/// unrouted and a removed route cannot leave the routed set behind.
#[test]
fn request_subcommands_match_the_registry_request_and_peer_subset() {
    let mut derived: Vec<&str> = compliance_subcommands()
        .into_iter()
        .filter(|sub| sub.starts_with("request-") || sub.starts_with("peer-"))
        .collect();
    derived.sort_unstable();
    let mut guard = REQUEST_SUBCOMMANDS.to_vec();
    guard.sort_unstable();
    assert_eq!(
        guard, derived,
        "the routed verb list matches the registry subset"
    );
}

/// Run one request verb with no flags against a scratch store: every verb
/// requires `--request-id`, so its dispatch arm runs, opens the store, and
/// refuses with a usage error before any tool call. The scratch `--db` keeps
/// the open off the host's data directory.
fn run_verb_without_flags(sub: &str, db: &std::path::Path) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        super::run(
            &mut io,
            vec![
                (*sub).to_owned(),
                "--db".to_owned(),
                db.display().to_string(),
            ],
            Ok(std::env::temp_dir()),
        )
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// A unique scratch directory, removed when the guard drops.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "clio-compliance-routing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is set")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create the scratch dir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every routed request and peer-request verb reaches its own dispatch arm:
/// each call with no flags is a usage refusal naming the verb's required
/// `--request-id`, never an unknown-subcommand refusal.
#[test]
fn request_verbs_dispatch_to_their_own_arms() {
    let scratch = Scratch::new();
    let db = scratch.0.join("store.db");
    for sub in REQUEST_SUBCOMMANDS {
        let (code, _out, err) = run_verb_without_flags(sub, &db);
        assert_eq!(
            code,
            ExitCode::from(EXIT_USAGE),
            "{sub} refuses without its flags: {err}"
        );
        assert!(
            err.contains("--request-id"),
            "{sub} reaches its own parser: {err}"
        );
        assert!(
            !err.contains("unknown `compliance` subcommand"),
            "{sub} is routed, not refused as unknown: {err}"
        );
    }
}
