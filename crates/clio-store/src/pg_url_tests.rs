// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing how the SSL options of a connection
//! URL are read: the mode, the optional root certificate, and the URL the
//! driver's own parser is handed.
//!
//! ## Owns
//! - The mode-to-driver mapping for every mode
//! - The mode a string with no `sslmode` defaults to, from its hosts
//! - Keeping the other query parameters, and dropping `sslrootcert`
//! - Percent-decoding a supplied root path
//!
//! ## Does not own
//! - Loading the root (`tls_trust` owns that)
//! - Connecting (`pg_tls` owns that)
//!
//! ## Boundary
//!
//! Changes to how an SSL option is spelled in a URL should normally require
//! changes here.

use super::*;
use crate::pg_tls::SslChoice;

#[test]
fn the_driver_url_maps_the_mode_and_drops_the_root() {
    let spec = spec("postgres://u:p@h:5432/db?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem")
        .expect("a supported mode and a root");
    assert_eq!(spec.ssl, SslChoice::VerifyFull);
    assert_eq!(spec.root.as_deref(), Some("/etc/ca.pem"));
    assert_eq!(spec.driver_url, "postgres://u:p@h:5432/db?sslmode=require");
}

#[test]
fn every_mode_maps_to_the_driver_value_that_forces_the_right_negotiation() {
    for (mode, driver) in [
        ("disable", "disable"),
        ("prefer", "prefer"),
        ("verify-full", "require"),
        ("encrypt-no-verify", "require"),
    ] {
        let spec = spec(&format!("postgres://u@h/db?sslmode={mode}")).expect("a supported mode");
        assert_eq!(
            spec.driver_url,
            format!("postgres://u@h/db?sslmode={driver}"),
            "sslmode={mode}"
        );
    }
}

#[test]
fn other_query_parameters_are_kept_and_the_mode_is_appended() {
    let spec =
        spec("postgres://u@h/db?application_name=clio&sslmode=encrypt-no-verify&connect_timeout=5")
            .expect("a supported mode");
    assert_eq!(spec.ssl, SslChoice::EncryptNoVerify);
    assert_eq!(spec.root, None);
    assert_eq!(
        spec.driver_url,
        "postgres://u@h/db?application_name=clio&connect_timeout=5&sslmode=require"
    );
}

#[test]
fn a_url_with_no_mode_requires_tls_for_a_remote_host() {
    let remote = spec("postgres://u@db.example.com/db").expect("no mode names a default");
    assert_eq!(remote.ssl, SslChoice::VerifyFull);
    assert_eq!(
        remote.driver_url,
        "postgres://u@db.example.com/db?sslmode=require"
    );

    // One remote host in a list is enough to require TLS for the whole string.
    let mixed = spec("postgres://u@127.0.0.1,db.example.com/db").expect("a host list is valid");
    assert_eq!(mixed.ssl, SslChoice::VerifyFull);
}

#[test]
fn a_query_with_no_mode_still_requires_tls_for_a_remote_host() {
    let spec = spec("postgres://u@db.example.com/db?application_name=clio")
        .expect("no mode names a default");
    assert_eq!(spec.ssl, SslChoice::VerifyFull);
    assert_eq!(
        spec.driver_url,
        "postgres://u@db.example.com/db?application_name=clio&sslmode=require"
    );
}

#[test]
fn a_url_with_no_mode_keeps_prefer_for_a_local_host() {
    for url in [
        "postgres://u@localhost/db",
        "postgres://u@127.0.0.1:5432/db",
        "postgres://u@[::1]/db",
        "postgres://u@%2Fvar%2Frun%2Fpostgresql/db",
        "postgres:///db",
    ] {
        let spec = spec(url).expect("a local address needs no TLS");
        assert_eq!(spec.ssl, SslChoice::Prefer, "{url}");
        assert_eq!(spec.driver_url, url, "{url}");
    }
}

#[test]
fn an_explicit_mode_wins_over_the_host_default() {
    let remote = spec("postgres://u@db.example.com/db?sslmode=prefer").expect("an explicit mode");
    assert_eq!(remote.ssl, SslChoice::Prefer);
    assert_eq!(
        remote.driver_url,
        "postgres://u@db.example.com/db?sslmode=prefer"
    );

    let local = spec("postgres://u@localhost/db?sslmode=verify-full").expect("an explicit mode");
    assert_eq!(local.ssl, SslChoice::VerifyFull);
    assert_eq!(
        local.driver_url,
        "postgres://u@localhost/db?sslmode=require"
    );
}

#[test]
fn a_key_value_string_keeps_its_mode_and_has_no_root() {
    let spec = spec("host=h user=u sslmode=prefer").expect("a supported mode");
    assert_eq!(spec.ssl, SslChoice::Prefer);
    assert_eq!(spec.root, None);
    // The string is left for the driver, which is the only reader of this form.
    assert_eq!(spec.driver_url, "host=h user=u sslmode=prefer");
}

#[test]
fn a_key_value_string_with_no_mode_follows_its_host() {
    let remote = spec("host=db.example.com user=u").expect("no mode names a default");
    assert_eq!(remote.ssl, SslChoice::VerifyFull);
    assert_eq!(
        remote.driver_url,
        "host=db.example.com user=u sslmode=require"
    );

    let local = spec("host=127.0.0.1 user=u").expect("a local address needs no TLS");
    assert_eq!(local.ssl, SslChoice::Prefer);
    assert_eq!(local.driver_url, "host=127.0.0.1 user=u");
}

#[test]
fn an_unsupported_mode_is_refused_with_the_store_message() {
    let err = spec("postgres://u@h/db?sslmode=require").expect_err("require is not offered");
    assert!(err.contains("verify-full"), "{err}");
    assert!(err.contains("encrypt-no-verify"), "{err}");
}

#[test]
fn an_empty_root_is_no_root() {
    let spec = spec("postgres://u@h/db?sslrootcert=").expect("an empty root is not a mode error");
    assert_eq!(spec.root, None);
}

#[test]
fn a_partial_escape_is_kept_as_written() {
    assert_eq!(percent_decode("a%2"), "a%2");
    assert_eq!(percent_decode("a%zz"), "a%zz");
    assert_eq!(percent_decode("%2F"), "/");
    assert_eq!(percent_decode("plain"), "plain");
}
