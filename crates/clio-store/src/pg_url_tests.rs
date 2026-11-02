// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing how the SSL options of a connection
//! string are read: the mode, the optional root certificate, and the string the
//! driver's own parser is handed.
//!
//! ## Owns
//! - The mode-to-driver mapping for every mode
//! - The mode a string with no `sslmode` defaults to, from every effective host
//! - Keeping the other parameters, and dropping only the store's two keys
//! - Percent-decoding a supplied root path
//!
//! ## Does not own
//! - Loading the root (`tls_trust` owns that)
//! - Connecting (`pg_tls` owns that)
//!
//! ## Boundary
//!
//! Changes to how an SSL option is spelled in a connection string should
//! normally require changes here.

use super::*;
use crate::pg_tls::SslChoice;
use postgres::config::SslMode;

#[test]
fn the_driver_string_drops_only_the_store_keys() {
    let spec = spec("postgres://u:p@h:5432/db?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem")
        .expect("a supported mode and a root");
    assert_eq!(spec.ssl, SslChoice::VerifyFull);
    assert_eq!(spec.root.as_deref(), Some("/etc/ca.pem"));
    assert_eq!(spec.driver_url, "postgres://u:p@h:5432/db");
    assert_eq!(driver_mode(spec.ssl), SslMode::Require);
}

#[test]
fn every_mode_maps_to_the_driver_value_that_forces_the_right_negotiation() {
    for (mode, expected) in [
        ("disable", SslMode::Disable),
        ("prefer", SslMode::Prefer),
        ("verify-full", SslMode::Require),
        ("encrypt-no-verify", SslMode::Require),
    ] {
        let spec = spec(&format!("postgres://u@h/db?sslmode={mode}")).expect("a supported mode");
        assert_eq!(spec.driver_url, "postgres://u@h/db", "sslmode={mode}");
        assert_eq!(driver_mode(spec.ssl), expected, "sslmode={mode}");
    }
}

#[test]
fn other_query_parameters_are_kept_in_their_order() {
    let spec =
        spec("postgres://u@h/db?application_name=clio&sslmode=encrypt-no-verify&connect_timeout=5")
            .expect("a supported mode");
    assert_eq!(spec.ssl, SslChoice::EncryptNoVerify);
    assert_eq!(spec.root, None);
    assert_eq!(
        spec.driver_url,
        "postgres://u@h/db?application_name=clio&connect_timeout=5"
    );
}

#[test]
fn a_url_with_no_mode_requires_tls_for_a_remote_host() {
    for url in [
        "postgres://u@db.example.com/db",
        "postgres://u@db.example.com/db?application_name=clio",
    ] {
        let spec = spec(url).expect("no mode names a default");
        assert_eq!(spec.ssl, SslChoice::VerifyFull, "{url}");
        assert_eq!(spec.driver_url, url, "nothing but the mode is rewritten");
    }

    // One remote host in a list is enough to require TLS for the whole string.
    let mixed = spec("postgres://u@127.0.0.1,db.example.com/db").expect("a host list is valid");
    assert_eq!(mixed.ssl, SslChoice::VerifyFull);
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

    let mixed = spec("postgres://u@127.0.0.1,localhost/db").expect("a local host list");
    assert_eq!(mixed.ssl, SslChoice::Prefer);
}

#[test]
fn a_query_declared_host_is_what_decides() {
    // The driver accepts `host` and `hostaddr` in the query, and both name a
    // TCP address this default has to see.
    for url in [
        "postgresql:///mydb?host=db.example.com",
        "postgresql:///mydb?hostaddr=10.0.0.5",
    ] {
        let spec = spec(url).expect("the query is valid");
        assert_eq!(spec.ssl, SslChoice::VerifyFull, "{url}");
    }

    // `hostaddr` is the address the driver dials, so it wins over the name.
    let remote =
        spec("postgresql:///mydb?host=127.0.0.1&hostaddr=10.0.0.5").expect("both keys are valid");
    assert_eq!(remote.ssl, SslChoice::VerifyFull);

    let local = spec("postgresql:///mydb?host=db.example.com&hostaddr=127.0.0.1")
        .expect("both keys are valid");
    assert_eq!(local.ssl, SslChoice::Prefer);
}

#[test]
fn a_mixed_address_list_is_remote_when_any_address_is() {
    for input in [
        "postgresql:///mydb?hostaddr=127.0.0.1,10.0.0.5",
        "host=127.0.0.1,db.example.com user=u",
    ] {
        let spec = spec(input).expect("a mixed list is valid");
        assert_eq!(spec.ssl, SslChoice::VerifyFull, "{input}");
    }
}

#[test]
fn a_unix_socket_is_local_and_a_list_with_a_remote_host_is_not() {
    let socket = spec("postgresql:///mydb?host=/var/run/postgresql").expect("a socket is valid");
    assert_eq!(socket.ssl, SslChoice::Prefer);

    let mixed = spec("host=/var/run/postgresql,db.example.com user=u").expect("a mixed list");
    assert_eq!(mixed.ssl, SslChoice::VerifyFull);
}

#[test]
fn an_explicit_mode_wins_over_the_host_default() {
    let remote = spec("postgres://u@db.example.com/db?sslmode=prefer").expect("an explicit mode");
    assert_eq!(remote.ssl, SslChoice::Prefer);

    let local = spec("postgres://u@localhost/db?sslmode=verify-full").expect("an explicit mode");
    assert_eq!(local.ssl, SslChoice::VerifyFull);

    let off = spec("postgres://u@db.example.com/db?sslmode=disable").expect("an explicit mode");
    assert_eq!(off.ssl, SslChoice::Disable);
}

#[test]
fn a_key_value_string_keeps_its_parameters_and_has_no_root() {
    let spec = spec("host=h user=u sslmode=prefer").expect("a supported mode");
    assert_eq!(spec.ssl, SslChoice::Prefer);
    assert_eq!(spec.root, None);
    assert_eq!(spec.driver_url, "host=h user=u");
}

#[test]
fn a_key_value_string_with_no_mode_follows_its_host() {
    let remote = spec("host=db.example.com user=u").expect("no mode names a default");
    assert_eq!(remote.ssl, SslChoice::VerifyFull);
    assert_eq!(remote.driver_url, "host=db.example.com user=u");

    let local = spec("host=127.0.0.1 user=u").expect("a local address needs no TLS");
    assert_eq!(local.ssl, SslChoice::Prefer);
    assert_eq!(local.driver_url, "host=127.0.0.1 user=u");
}

#[test]
fn a_key_value_mode_is_found_across_any_whitespace_and_around_the_equals() {
    for input in [
        "host=db.example.com user=u\tsslmode=disable",
        "host=db.example.com user=u  sslmode=disable",
        "host=db.example.com user=u sslmode = disable",
        "host=db.example.com user=u sslmode\t=\tdisable",
    ] {
        let spec = spec(input).expect("the mode is present");
        assert_eq!(spec.ssl, SslChoice::Disable, "{input}");
        assert_eq!(spec.driver_url, "host=db.example.com user=u", "{input}");
    }
}

#[test]
fn a_key_value_explicit_mode_wins_over_a_remote_host() {
    let spec = spec("host=db.example.com sslmode=prefer").expect("an explicit mode");
    assert_eq!(spec.ssl, SslChoice::Prefer);
}

#[test]
fn a_password_cannot_move_the_query_or_the_mode() {
    // A raw `?`, an encoded one, `&`, and `://` all belong to the password.
    for url in [
        "postgres://u:p?ss@db.example.com/db",
        "postgres://u:p%3Fss@db.example.com/db",
        "postgres://u:p&ss@db.example.com/db",
        "postgres://u:p://ss@db.example.com/db",
    ] {
        let parsed = spec(url).expect("the driver reads this password");
        assert_eq!(parsed.ssl, SslChoice::VerifyFull, "{url}");
        assert_eq!(parsed.driver_url, url, "{url}");
    }

    // A password that looks like an option is still a password.
    let url = "postgres://u:p?sslrootcert=%2Fetc@db.example.com/db";
    let parsed = spec(url).expect("the driver reads this password");
    assert_eq!(parsed.root, None);
    assert_eq!(parsed.driver_url, url);

    // An explicit mode after such a password still reaches the mode.
    let explicit = spec("postgres://u:p?ss&word@db.example.com/db?sslmode=prefer")
        .expect("the mode is after the real query mark");
    assert_eq!(explicit.ssl, SslChoice::Prefer);
    assert_eq!(
        explicit.driver_url,
        "postgres://u:p?ss&word@db.example.com/db"
    );
}

#[test]
fn an_unsupported_mode_is_refused_with_the_store_message() {
    for input in [
        "postgres://u@h/db?sslmode=require",
        "host=h sslmode=require",
    ] {
        let err = spec(input).expect_err("require is not offered");
        assert!(err.contains("verify-full"), "{input}: {err}");
        assert!(err.contains("encrypt-no-verify"), "{input}: {err}");
    }
}

#[test]
fn an_empty_root_is_no_root() {
    let spec = spec("postgres://u@h/db?sslrootcert=").expect("an empty root is not a mode error");
    assert_eq!(spec.root, None);
    assert_eq!(spec.driver_url, "postgres://u@h/db");
}

#[test]
fn a_partial_escape_is_kept_as_written() {
    assert_eq!(percent_decode("a%2"), "a%2");
    assert_eq!(percent_decode("a%zz"), "a%zz");
    assert_eq!(percent_decode("%2F"), "/");
    assert_eq!(percent_decode("plain"), "plain");
}
