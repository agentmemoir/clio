// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio compliance` durable-request bindings: the
//! records and their tool bindings, flag parsing and refusal, and one
//! end-to-end start-then-status flow that renders the separated request fields.
//!
//! ## Owns
//! - Behavior, refusal, and registry tests for `compliance_request_cli`
//!
//! ## Does not own
//! - The command under test (`compliance_request_cli`)
//! - The request state machine and the durable stores (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::cli_error::{EXIT_OPERATIONAL, EXIT_USAGE};
use crate::cli_read_test_support::{json_of, state, tokens};

/// Run one request verb against injected state.
fn run_injected(sub: &str, args: &[&str], state: &Arc<McpState>) -> (ExitCode, String, String) {
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
        super::run_with_state(sub, tokens(args), state, &mut io)
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// Every request record binds its matching tool and its required flag set.
#[test]
fn compliance_request_records_bind_their_tools() {
    let cases: [(&str, &str, &[&str]); 10] = [
        (
            "compliance request-start",
            "subject_request_start",
            &["request-id", "kind"],
        ),
        (
            "compliance request-status",
            "subject_request_status",
            &["request-id"],
        ),
        (
            "compliance request-retry",
            "subject_request_retry",
            &["request-id", "subject-id", "legal-basis"],
        ),
        (
            "compliance request-copy",
            "subject_request_copy",
            &[
                "copy-id",
                "request-id",
                "custody",
                "owner",
                "required-action",
                "state",
            ],
        ),
        (
            "compliance request-exception",
            "subject_request_exception",
            &["request-id", "reason"],
        ),
        (
            "compliance request-events",
            "subject_request_events",
            &["request-id"],
        ),
        (
            "compliance request-close",
            "subject_request_close",
            &["request-id", "product-path"],
        ),
        (
            "compliance peer-status",
            "subject_request_peer_status",
            &["request-id"],
        ),
        (
            "compliance peer-retry",
            "subject_request_peer_retry",
            &["request-id"],
        ),
        (
            "compliance peer-disposition",
            "subject_request_peer_disposition",
            &["request-id", "peer-id", "disposition"],
        ),
    ];
    for (name, tool, required) in cases {
        let spec = crate::cli_command_registry::find(name)
            .unwrap_or_else(|| panic!("`{name}` is registered"));
        assert_eq!(spec.tool, Some(tool), "`{name}` binds its tool");
        assert_eq!(spec.required_flags(), required, "`{name}` required flags");
    }
}

/// A missing required flag and an unknown flag both exit 2 with a message.
#[test]
fn compliance_request_usage_refusals_exit_two() {
    let s = state();
    let missing: [(&str, &[&str]); 3] = [
        ("request-status", &[]),
        ("request-start", &["--kind", "erasure"]),
        (
            "peer-disposition",
            &["--request-id", "r1", "--peer-id", "p1"],
        ),
    ];
    for (sub, args) in missing {
        let (code, out, err) = run_injected(sub, args, &s);
        assert_eq!(code, ExitCode::from(EXIT_USAGE), "{sub}: {out}{err}");
        assert!(!err.trim().is_empty(), "{sub} wrote no error");
    }
    let (code, _out, err) = run_injected("request-status", &["--bogus", "x"], &s);
    assert_eq!(code, ExitCode::from(EXIT_USAGE), "{err}");
    assert!(
        err.contains("bogus"),
        "the unknown flag must be named: {err}"
    );
}

/// `request-retry` without `--confirm` is refused by the tool and mutates
/// nothing.
#[test]
fn compliance_request_retry_without_confirm_is_refused() {
    let s = state();
    let (code, out, err) = run_injected(
        "request-retry",
        &[
            "--request-id",
            "r1",
            "--subject-id",
            "s1",
            "--legal-basis",
            "B",
            "-o",
            "json",
        ],
        &s,
    );
    assert_eq!(code, ExitCode::from(EXIT_OPERATIONAL), "{out}{err}");
    let payload = json_of(&err);
    assert_eq!(payload["ok"], serde_json::json!(false), "{payload}");
    assert_eq!(payload["code"], serde_json::json!("forbidden"), "{payload}");
}

/// The full flow: `request-start` opens a request, `request-status` reads it
/// back, and the text view prints the three separated fields.
#[test]
fn compliance_request_start_then_status_reports_the_separated_fields() {
    let s = state();
    let (code, out, err) = run_injected(
        "request-start",
        &["--request-id", "r1", "--kind", "erasure", "-o", "json"],
        &s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let started = json_of(&out);
    assert_eq!(started["ok"], serde_json::json!(true), "{started}");
    assert_eq!(started["request_id"], serde_json::json!("r1"), "{started}");
    assert_eq!(
        started["technical_completion"],
        serde_json::json!("pending"),
        "{started}"
    );
    assert_eq!(
        started["request_outcome"],
        serde_json::json!("open"),
        "{started}"
    );
    assert!(
        started["pending_operator_actions"].is_array(),
        "the pending actions field must be its own array: {started}"
    );

    let (code, out, err) =
        run_injected("request-status", &["--request-id", "r1", "-o", "json"], &s);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let reported = json_of(&out);
    assert_eq!(
        reported["request_id"],
        serde_json::json!("r1"),
        "{reported}"
    );
    assert_eq!(
        reported["state"],
        serde_json::json!("InProgress"),
        "{reported}"
    );

    let (code, text, err) =
        run_injected("request-status", &["--request-id", "r1", "-o", "text"], &s);
    assert_eq!(code, ExitCode::SUCCESS, "{text}{err}");
    assert_eq!(
        text,
        "request r1: state=InProgress; technical_completion=pending; \
         pending_operator_actions=none; request_outcome=open\n",
        "{text}"
    );
}

/// `request-start` round-trips the operator verification decision and the
/// optional deadline, and the stored terms are reported by start and status.
#[test]
fn compliance_request_start_records_verification_and_deadline() {
    let s = state();
    let (code, out, err) = run_injected(
        "request-start",
        &[
            "--request-id",
            "r-terms",
            "--kind",
            "erasure",
            "--verification",
            "verified",
            "--deadline",
            "2026-03-01T00:00:00Z",
            "-o",
            "json",
        ],
        &s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let started = json_of(&out);
    assert_eq!(
        started["verification"],
        serde_json::json!("verified"),
        "{started}"
    );
    assert_eq!(
        started["deadline"],
        serde_json::json!("2026-03-01T00:00:00Z"),
        "{started}"
    );

    let (code, out, err) = run_injected(
        "request-status",
        &["--request-id", "r-terms", "-o", "json"],
        &s,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let reported = json_of(&out);
    assert_eq!(
        reported["verification"],
        serde_json::json!("verified"),
        "{reported}"
    );
    assert_eq!(
        reported["deadline"],
        serde_json::json!("2026-03-01T00:00:00Z"),
        "{reported}"
    );
}
