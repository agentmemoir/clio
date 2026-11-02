// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the real setup probes: the database
//! detail's refusal of a database without pgvector, and the endpoint probe's
//! pass-through of the index transport's own message.
//!
//! ## Owns
//! - Probe formatting and refusal behavior under test for `setup_probes`
//!
//! ## Does not own
//! - Production probe logic under test
//! - The store's connection test (`clio-store` owns it)
//!
//! ## Boundary
//!
//! Changes to the database detail's wording or to the vector refusal should
//! normally update tests here. Changes to the transport should not.
//!
//! Keep this module focused on the probe's own formatting.
//!
//! # Why no connection is opened
//!
//! A crafted [`DatabaseReport`] reaches every branch of the decision without a
//! server, and the endpoint check is handed a URL the request gate refuses
//! before anything is dialled: a probe test that needed a live service would be
//! a test of the machine, not of the code.

use super::*;

fn report(version: &str, has_vector: bool) -> DatabaseReport {
    DatabaseReport {
        server_version: version.to_owned(),
        has_vector,
        server_tls: false,
        server_tls_version: None,
        server_tls_cipher: None,
        requested_tls: SslChoice::Prefer,
    }
}

#[test]
fn a_vector_enabled_database_reports_the_transcript_detail() {
    let detail = database_detail(&report("16.4", true)).expect("a vector-enabled database passes");
    assert_eq!(
        detail,
        "connected, PostgreSQL 16.4, vector extension present, client asks for TLS and allows \
         cleartext, server reports no TLS"
    );
}

#[test]
fn an_encrypted_connection_reports_the_negotiated_version_and_cipher() {
    let detail = database_detail(&DatabaseReport {
        server_version: "16.4".to_owned(),
        has_vector: true,
        server_tls: true,
        server_tls_version: Some("TLSv1.3".to_owned()),
        server_tls_cipher: Some("TLS_AES_256_GCM_SHA384".to_owned()),
        requested_tls: SslChoice::VerifyFull,
    })
    .expect("a vector-enabled database passes");
    assert_eq!(
        detail,
        "connected, PostgreSQL 16.4, vector extension present, client requires verified TLS, \
         server reports TLS with TLSv1.3 using TLS_AES_256_GCM_SHA384"
    );
}

#[test]
fn the_two_legs_are_named_separately_when_they_disagree() {
    // A pooler that terminates TLS: the client leg is plaintext while the
    // server's own leg reports TLS. Neither answer may stand for both.
    let detail = database_detail(&DatabaseReport {
        server_version: "16.4".to_owned(),
        has_vector: true,
        server_tls: true,
        server_tls_version: None,
        server_tls_cipher: None,
        requested_tls: SslChoice::Disable,
    })
    .expect("a vector-enabled database passes");
    assert_eq!(
        detail,
        "connected, PostgreSQL 16.4, vector extension present, client uses no TLS, server reports \
         TLS"
    );
}

#[test]
fn a_server_that_reports_tls_without_details_says_only_that() {
    let cases: [(Option<&str>, Option<&str>); 3] = [
        (None, None),
        (Some("TLSv1.3"), None),
        (None, Some("TLS_AES_256_GCM_SHA384")),
    ];
    for (version, cipher) in cases {
        let detail = database_detail(&DatabaseReport {
            server_version: "16.4".to_owned(),
            has_vector: true,
            server_tls: true,
            server_tls_version: version.map(ToOwned::to_owned),
            server_tls_cipher: cipher.map(ToOwned::to_owned),
            requested_tls: SslChoice::Disable,
        })
        .expect("a vector-enabled database passes");
        assert_eq!(
            detail,
            "connected, PostgreSQL 16.4, vector extension present, client uses no TLS, server \
             reports TLS"
        );
    }
}

#[test]
fn a_database_without_the_vector_extension_is_refused() {
    let error = database_detail(&report("16.4", false)).expect_err("no vector");
    assert!(error.contains("vector"), "{error}");
    assert!(error.contains("CREATE EXTENSION vector"), "{error}");
    assert!(
        error.contains("cannot store memories"),
        "the refusal explains why it matters: {error}"
    );
}

#[test]
fn the_endpoint_probe_maps_the_transport_message_through_unchanged() {
    // A scheme the request gate refuses before any connection is attempted.
    let url = "ftp://embed.example.com";
    let from_index =
        clio_index::probe_endpoint(url, None).expect_err("the gate refuses a non-http scheme");
    let from_probe = real_probes()
        .test_endpoint(url, None)
        .expect_err("the gate refuses a non-http scheme");
    assert_eq!(from_probe, from_index.message);
    assert!(!from_probe.is_empty(), "the message is the actionable half");
}
