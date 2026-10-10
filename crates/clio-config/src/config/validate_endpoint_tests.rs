// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the config-time rule that a remote
//! service endpoint must be `https`, and for the cases it must leave alone.
//!
//! ## Owns
//! - Remote plain `http` refused on every endpoint path, with the reason
//! - Loopback and `https` accepted unchanged
//! - The paths this rule does not touch, including `store.database_url`
//!
//! ## Does not own
//! - The rest of the path/value matrix (`validate_tests`)
//! - The request-time gate in `clio-index`, which is the second half of this
//!
//! ## Boundary
//!
//! Changes to the endpoint rule or the paths it covers should normally require
//! changes here. Changes to the parse or the loopback definition should not
//! require changes here; those are tested where they live.

use super::{ErrorCode, validate_set_value};
use clio_types::is_loopback_host;
use serde_json::json;

/// Every path whose value is a service endpoint and so carries an API key.
const ENDPOINT_PATHS: &[&str] = &["embed.url", "rerank.url", "extract.url"];

#[test]
fn a_remote_plain_http_endpoint_is_refused_on_every_endpoint_path() {
    for path in ENDPOINT_PATHS {
        let err = validate_set_value(path, &json!("http://api.example.com"))
            .expect_err("a remote plain-http endpoint must be refused");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{path}");
        let text = err.to_string();
        // The reason is the point: an operator has to be told what would cross
        // the network, not just that a rule fired.
        assert!(text.contains("API key"), "{path}: {text}");
        assert!(text.contains("unencrypted"), "{path}: {text}");
        // And what to do instead.
        assert!(text.contains("https://api.example.com"), "{path}: {text}");
        assert!(text.contains("127.0.0.1"), "{path}: {text}");
    }
}

#[test]
fn a_remote_https_endpoint_is_accepted() {
    for path in ENDPOINT_PATHS {
        validate_set_value(path, &json!("https://api.example.com")).unwrap();
        validate_set_value(path, &json!("https://api.example.com:8443/v1/embed")).unwrap();
    }
}

#[test]
fn a_loopback_endpoint_stays_on_plain_http() {
    // The local sidecars are on this machine, which is the whole reason they can
    // stay on http. Refusing these would make a working local setup impossible.
    for host in ["localhost:34311", "127.0.0.1:34311", "box.localhost:34311"] {
        for path in ENDPOINT_PATHS {
            validate_set_value(path, &json!(format!("http://{host}"))).unwrap();
        }
    }
    for path in ENDPOINT_PATHS {
        validate_set_value(path, &json!("http://[::1]:34311")).unwrap();
    }
}

#[test]
fn the_host_is_read_from_the_url_not_from_the_whole_string() {
    // A userinfo prefix is not a host. Reading the host as everything up to the
    // first slash would call this `trusted`, which is not loopback, and reading
    // it as the whole authority would call it `trusted@box.localhost`, which is.
    for path in ENDPOINT_PATHS {
        let err = validate_set_value(path, &json!("http://trusted@evil.example/v1"))
            .expect_err("userinfo must not hide the real host");
        assert!(err.to_string().contains("evil.example"), "{path}: {err}");
    }
    // And the loopback name in front of a remote host does not save it.
    for path in ENDPOINT_PATHS {
        validate_set_value(path, &json!("http://box.localhost@evil.example/v1"))
            .expect_err("a loopback-looking userinfo is not a loopback host");
    }
}

#[test]
fn a_non_http_scheme_is_left_to_the_rules_that_already_existed() {
    // This rule speaks only about plain http to a remote host. Everything else
    // keeps whatever behaviour it had, so nothing new is refused here.
    for value in [
        "",
        "not-a-url",
        "localhost:34311",
        "ftp://files.example.com",
        "postgres://db.example.com/clio",
    ] {
        validate_set_value("embed.url", &json!(value))
            .unwrap_or_else(|e| panic!("`{value}` must keep its previous behaviour: {e}"));
    }
}

#[test]
fn a_non_string_endpoint_is_still_a_type_error() {
    for path in ENDPOINT_PATHS {
        let err = validate_set_value(path, &json!(34311)).expect_err("not a string");
        assert_eq!(err.code, ErrorCode::InvalidType, "{path}");
    }
}

#[test]
fn the_two_gates_never_disagree_about_the_same_url() {
    // The config gate reads a stored value; the request gate reads one the
    // transport parsed. For a value both can read as an http/https URL they must
    // reach the same verdict from the same shared rule. A fourth copy of the
    // loopback list is the failure this prevents, and this is what catches it if
    // one of the two ever stops asking the rule.
    for url in [
        "http://api.example.com/embed",
        "http://trusted@api.example.com/embed",
        "http://box.localhost@api.example.com/embed",
        "http://localhost:34311",
        "http://127.0.0.1:34311",
        "http://127.9.9.9:34311",
        "http://box.localhost:34311",
        "http://[::1]:34311",
        "http://[::1]",
        "https://api.example.com",
        "https://api.example.com:8443/v1/embed",
    ] {
        let config_ok = validate_set_value("embed.url", &json!(url)).is_ok();
        // The request gate, as `clio-index` writes it: `https` passes, and
        // anything else must be loopback to be sent.
        let request_ok = url.starts_with("https://")
            || clio_types::parse_scheme_host(url)
                .is_ok_and(|(scheme, host)| scheme != "http" || is_loopback_host(&host));
        assert_eq!(
            config_ok, request_ok,
            "the config gate and the request gate disagree about `{url}`"
        );
    }
}

#[test]
fn an_unreadable_value_is_left_to_the_request_gate_on_purpose() {
    // The config gate speaks only about plain http to a remote host, and
    // deliberately does not judge the rest of a value: a value it cannot read as
    // a URL is the request gate's to refuse, and that gate owns the parse. So the
    // two are *meant* to differ here, and this asserts that the difference is
    // still the designed one rather than an accident.
    for value in ["not-a-url", "", "localhost:34311", "ftp://api.example.com"] {
        assert!(
            validate_set_value("embed.url", &json!(value)).is_ok(),
            "`{value}` is not this rule's business"
        );
    }
}

#[test]
fn the_database_url_is_a_postgres_check_not_an_http_check() {
    // `store.database_url` is a Postgres connection string. This rule must not
    // read it as an endpoint, and must not start judging its shape: the store
    // asks the Postgres driver, which is the parser of record.
    validate_set_value(
        "store.database_url",
        &json!("postgres://clio:clio@db.example.com:5432/clio?sslmode=require"),
    )
    .expect("a postgres url is not an http endpoint");
    validate_set_value("store.database_url", &json!("")).expect("still only a string check");
    assert_eq!(
        validate_set_value("store.database_url", &json!(7))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
}
