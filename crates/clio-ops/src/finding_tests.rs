// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the PII-safe operator finding
//! shape: severity ordering, aggregate `ok` behavior, and scope labels.
//!
//! ## Owns
//! - `Severity` ordering and `Findings` aggregation edge cases
//! - `OpsFinding` construction, `scope_label` formatting, and the credential redactor
//!
//! ## Does not own
//! - Check logic that emits findings (`diagnose` / `verify`)
//! - Repair planning or the action allowlist (`doctor` / `repair`)
//!
//! ## Boundary
//!
//! Changes to the finding shape or code catalog should normally update tests
//! here. Changes to check logic should not require changes here.
//!
//! Keep this module focused on the finding shape and catalog.

use crate::finding::{Findings, OpsFinding, Severity, codes, redact_credentials, scope_label};

fn finding(code: &str, severity: Severity) -> OpsFinding {
    OpsFinding::new(code, severity, "store", "msg")
}

#[test]
fn severity_orders_info_warning_error() {
    assert!(Severity::Info < Severity::Warning);
    assert!(Severity::Warning < Severity::Error);
}

#[test]
fn findings_aggregate_ok_only_fails_on_error() {
    let clean = Findings::new();
    assert!(clean.is_empty());
    let empty_len = clean.len();
    assert_eq!(empty_len, 0);
    assert!(clean.ok());
    assert_eq!(clean.max_severity(), None);

    let mut warnings = Findings::new();
    warnings.push(finding(codes::EMBEDDING_UNREACHABLE, Severity::Warning));
    assert!(!warnings.is_empty());
    assert!(warnings.ok());
    assert_eq!(warnings.max_severity(), Some(Severity::Warning));

    let mut errors = Findings::new();
    errors.push(finding(codes::MANIFEST_UNAVAILABLE, Severity::Info));
    errors.push(finding(codes::ORPHAN_ASSOC_EDGE, Severity::Error));
    assert!(!errors.ok());
    assert_eq!(errors.max_severity(), Some(Severity::Error));
    let emitted: Vec<String> = errors.into_vec().into_iter().map(|f| f.code).collect();
    assert_eq!(
        emitted,
        vec![
            codes::MANIFEST_UNAVAILABLE.to_owned(),
            codes::ORPHAN_ASSOC_EDGE.to_owned()
        ]
    );
}

#[test]
fn ops_finding_new_carries_fields() {
    let f = OpsFinding::new(
        codes::BACKEND_UNREACHABLE,
        Severity::Error,
        "bank:b",
        "down",
    );
    assert_eq!(f.code, codes::BACKEND_UNREACHABLE);
    assert_eq!(f.severity, Severity::Error);
    assert_eq!(f.scope, "bank:b");
    assert_eq!(f.message, "down");
}

#[test]
fn scope_label_renders_store_or_bank() {
    assert_eq!(scope_label(None), "store");
    assert_eq!(scope_label(Some("b1")), "bank:b1");
}

#[test]
fn redact_credentials_strips_url_userinfo_only() {
    let url = "connection failed for postgres://ops:hunter2@db.host:5432/am";
    assert_eq!(
        redact_credentials(url),
        "connection failed for postgres://***@db.host:5432/am"
    );
    assert_eq!(
        redact_credentials("https://example.com/x"),
        "https://example.com/x",
        "URL without userinfo is untouched"
    );
    assert_eq!(
        redact_credentials("plain error text"),
        "plain error text",
        "non-URL text passes through"
    );
}

#[test]
fn a_userinfo_with_no_password_is_not_redacted() {
    // The managed Postgres address is `postgres://clio@127.0.0.1:34310/clio`.
    // It has no password, so blanking its user name would put a redaction mark on
    // an address that never held a secret — and it would do so everywhere the
    // address is printed, including `clio status` and every error envelope.
    for address in [
        "postgres://clio@127.0.0.1:34310/clio",
        "connection failed for postgres://clio@127.0.0.1:34310/clio",
        "postgres://clio@127.0.0.1:34310/clio?sslmode=prefer",
    ] {
        assert_eq!(redact_credentials(address), address, "{address}");
    }
}

#[test]
fn an_empty_password_after_the_colon_is_still_redacted() {
    // A colon is the separator, so `user:@host` is a userinfo that names a
    // password position even when the value is empty. Redacting it costs nothing
    // and errs towards not printing a userinfo.
    assert_eq!(
        redact_credentials("postgres://clio:@db.host:5432/am"),
        "postgres://***@db.host:5432/am"
    );
}

#[test]
fn a_password_containing_an_at_sign_is_still_redacted() {
    // A percent-encoded `@` in a password cannot be mistaken for the userinfo
    // terminator, so the first `@` is the right one to split on.
    assert_eq!(
        redact_credentials("postgres://ops:p%40ss@db.host:5432/am"),
        "postgres://***@db.host:5432/am"
    );
}
