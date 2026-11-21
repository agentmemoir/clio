// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the credential rule: the server key is
//! offered only to an operator-allowlisted destination, and a pull with no
//! credential at any other destination is refused instead of sent bare.
//!
//! ## Owns
//! - The allowlist gate on the server key, in both the request header and the
//!   redacted report
//! - The fail-closed refusal for a credential-less pull, asserted by error code
//! - Caller-credential precedence, which the allowlist flag must not disturb
//!
//! ## Does not own
//! - Destination classification (`provider_destination_tests.rs`)
//! - The pull and its mapping (`provider_adapter_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_types::ErrorCode;
use serde_json::{Value, json};

use super::{ACCEPTED_CREDENTIAL_KEYS, authorization, extract_api_key, masked_credentials};

const ENV_KEY: &str = "env-secret-xyz9876";

/// A stand-in process environment: one variable, no ambient reads, so the suite
/// is host-independent and never mutates the real environment.
fn env(key: &str) -> Option<String> {
    (key == "CLIO_MEM0_API_KEY").then(|| ENV_KEY.to_owned())
}

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn the_server_key_is_offered_to_an_allowlisted_destination() {
    assert_eq!(
        authorization(&json!({}), &env, true),
        Ok(Some("Token env-secret-xyz9876".to_owned()))
    );
    assert_eq!(
        extract_api_key(&json!({}), &env, true).as_deref(),
        Some(ENV_KEY)
    );
}

#[test]
fn the_server_key_is_withheld_from_a_destination_off_the_allowlist() {
    assert_eq!(extract_api_key(&json!({}), &env, false), None);
}

#[test]
fn a_pull_off_the_allowlist_is_refused_with_or_without_a_credential() {
    for args in [
        json!({}),
        json!({"credentials": "direct-secret-str-5432"}),
        json!({"credentials": {"api_key": "obj-secret-9876"}}),
    ] {
        let err = authorization(&args, &env, false).expect_err("must refuse");
        assert_eq!(err.code, ErrorCode::Forbidden, "{err}");
        assert!(
            err.message.contains("CLIO_MEM0_API_KEY"),
            "the refusal names the key it withholds: {err}"
        );
    }
}

#[test]
fn an_allowlisted_destination_with_no_key_at_all_stays_reachable() {
    assert_eq!(authorization(&json!({}), &no_env, true), Ok(None));
}

#[test]
fn a_caller_credential_is_used_at_an_allowlisted_destination() {
    let args = json!({"credentials": "direct-secret-str-5432"});
    assert_eq!(
        authorization(&args, &no_env, true),
        Ok(Some("Token direct-secret-str-5432".to_owned()))
    );
    let object = json!({"credentials": {"api_key": "obj-secret-9876"}});
    assert_eq!(
        authorization(&object, &no_env, true),
        Ok(Some("Token obj-secret-9876".to_owned()))
    );
}

#[test]
fn a_prefixed_caller_credential_keeps_its_scheme() {
    let args = json!({"credentials": "Bearer bearer-secret-1234"});
    assert_eq!(
        authorization(&args, &no_env, true),
        Ok(Some("Bearer bearer-secret-1234".to_owned()))
    );
}

#[test]
fn the_report_claims_no_credential_off_the_allowlist() {
    assert_eq!(masked_credentials(&json!({}), &env, false), Value::Null);
    assert_eq!(
        masked_credentials(&json!({}), &env, true),
        json!({"api_key": "[REDACTED]"})
    );
}

#[test]
fn a_caller_credential_is_masked_whether_or_not_the_destination_is_allowlisted() {
    for sanctioned in [true, false] {
        assert_eq!(
            masked_credentials(
                &json!({"credentials": "direct-secret-str-5432"}),
                &env,
                sanctioned
            ),
            json!("[REDACTED]")
        );
        assert_eq!(
            masked_credentials(&json!({"credentials": null}), &env, sanctioned),
            Value::Null
        );
        assert_eq!(
            masked_credentials(&json!({"credentials": 42}), &env, sanctioned),
            json!(42)
        );
    }
}

/// Every key the extractor reads is a key the report masks, so a credential that
/// can reach the header can never be echoed in cleartext. `key` carries the probe
/// that exposed the drift: the name heuristics did not recognise it.
#[test]
fn every_accepted_credential_key_is_masked_in_the_report() {
    for key in ACCEPTED_CREDENTIAL_KEYS {
        let secret = "sk-live-CALLERSECRET";
        let mut object = serde_json::Map::new();
        object.insert((*key).to_owned(), json!(secret));
        let args = json!({"credentials": Value::Object(object)});
        assert_eq!(
            extract_api_key(&args, &no_env, true).as_deref(),
            Some(secret),
            "{key} is an accepted key"
        );
        let report = masked_credentials(&args, &no_env, true);
        assert_eq!(
            report.get(*key).and_then(Value::as_str),
            Some("[REDACTED]"),
            "{key} must be masked: {report}"
        );
        assert!(
            !report.to_string().contains(secret),
            "{key} leaked into the report: {report}"
        );
    }
}

/// A bare-string credential is the secret itself; the general JSON masker walks
/// objects only and left it untouched.
#[test]
fn a_bare_string_credential_is_masked() {
    for sanctioned in [true, false] {
        let report = masked_credentials(
            &json!({"credentials": "sk-live-STRINGKEY99"}),
            &no_env,
            sanctioned,
        );
        assert_eq!(report, json!("[REDACTED]"));
        assert!(!report.to_string().contains("STRINGKEY99"));
    }
}

#[test]
fn an_object_and_an_array_of_credentials_are_masked() {
    let object = masked_credentials(
        &json!({"credentials": {"key": "sk-live-K1", "token": "sk-live-T1"}}),
        &no_env,
        true,
    );
    assert_eq!(
        object,
        json!({"key": "[REDACTED]", "token": "[REDACTED]"}),
        "{object}"
    );
    let array = masked_credentials(
        &json!({"credentials": ["sk-live-A1", {"key": "sk-live-A2"}, 7]}),
        &no_env,
        true,
    );
    assert_eq!(
        array,
        json!(["[REDACTED]", {"key": "[REDACTED]"}, 7]),
        "{array}"
    );
    let text = array.to_string() + &object.to_string();
    assert!(!text.contains("sk-live"), "{text}");
}

/// Masking must not mangle what is not a credential: an unaccepted field keeps
/// its ordinary value, including a count that a name heuristic would eat.
#[test]
fn a_non_secret_field_is_not_mangled() {
    let report = masked_credentials(
        &json!({"credentials": {"key": "sk-live-K1", "region": "us-east-1", "max_tokens": 64}}),
        &no_env,
        true,
    );
    assert_eq!(
        report,
        json!({"key": "[REDACTED]", "region": "us-east-1", "max_tokens": 64}),
        "{report}"
    );
}

/// The unsupported-provider stub is the other live path for the same value:
/// `{"provider":"openai","credentials":"sk-live-..."}` reported the string as-is
/// because the general masker only walks objects.
#[test]
fn the_stub_report_masks_a_bare_string_credential() {
    let args = json!({"provider": "openai", "credentials": "sk-live-STRINGKEY99"});
    let reported = crate::portability_tools::masked_credentials(&args);
    assert_eq!(reported, json!("[REDACTED]"), "{reported}");
    assert!(!reported.to_string().contains("STRINGKEY99"));
}

/// The hard rename, pinned: the prefixed name is the only one read, and the bare
/// name resolves to nothing. A second spelling coming back is the failure this
/// test exists to catch.
#[test]
fn the_bare_provider_key_name_is_not_read() {
    let bare_only = |k: &str| (k == "MEM0_API_KEY").then(|| "bare-secret".to_owned());
    assert_eq!(extract_api_key(&json!({}), &bare_only, true), None);
    assert_eq!(
        masked_credentials(&json!({}), &bare_only, true),
        Value::Null
    );
}
