// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing how the store reads a Postgres
//! connection string and which connector each SSL mode picks. None of these
//! tests opens a socket.
//!
//! ## Owns
//! - Parse cases: schemes accepted and refused, host and port lists, defaults
//! - `sslmode` names, including the ones refused with a pointer to what works
//! - The local-or-remote answer for a parsed connection
//! - The connector each mode picks, read back off a parsed connection
//!
//! ## Does not own
//! - Reaching a real server (`pg_tls_db_tests` owns that)
//! - Store schema or query behavior
//! - The endpoint gates (`clio-config` and `clio-index` own those)
//!
//! ## Boundary
//!
//! Changes to the parse or the mode mapping should normally require changes
//! here. Changes to a query's result shape should not require changes here.

use super::{SslChoice, is_loopback, parse_connection};

#[test]
fn parses_a_postgres_url_with_its_ssl_mode() {
    let conn = parse_connection("postgres://u:p@db.example.com:5433/memories?sslmode=verify-full")
        .expect("verify-full is a supported mode");
    assert_eq!(conn.hosts, vec!["db.example.com".to_owned()]);
    assert_eq!(conn.ports, vec![5433]);
    assert_eq!(conn.database.as_deref(), Some("memories"));
    assert_eq!(conn.user.as_deref(), Some("u"));
    assert_eq!(conn.ssl, SslChoice::VerifyFull);
    assert!(!conn.is_local());

    // The other spelling of the scheme is the same thing.
    let conn = parse_connection("postgresql://u:p@localhost/clio").expect("postgresql is accepted");
    assert_eq!(conn.hosts, vec!["localhost".to_owned()]);
    assert_eq!(conn.database.as_deref(), Some("clio"));

    // The driver's own default port is filled in rather than left unknown.
    let conn = parse_connection("postgres://u:p@localhost/clio").expect("parse");
    assert_eq!(conn.ports, vec![5432]);
    assert_eq!(conn.primary_port(), 5432);
    assert_eq!(conn.primary_host(), "localhost");
}

#[test]
fn every_listed_host_and_port_keeps_its_order() {
    let conn = parse_connection("postgres://u:p@a.example.com:5433,b.example.com:5434/c")
        .expect("a comma-separated host list is valid");
    assert_eq!(
        conn.hosts,
        vec!["a.example.com".to_owned(), "b.example.com".to_owned()]
    );
    assert_eq!(conn.ports, vec![5433, 5434]);
    assert_eq!(conn.primary_host(), "a.example.com");
    assert_eq!(conn.primary_port(), 5433);
    // One remote host is enough to make the whole connection remote.
    assert!(!conn.is_local());

    let conn = parse_connection("postgres://u:p@127.0.0.1,localhost/c").expect("parse");
    assert!(conn.is_local());
}

#[test]
fn a_percent_encoded_password_is_the_drivers_to_read() {
    // The password is not in the output at all, so the parse must not choke on
    // the encoding and must not leak the secret it carries.
    let conn = parse_connection("postgres://u:p%40ss%2Fword@localhost/c").expect("parse");
    assert_eq!(conn.hosts, vec!["localhost".to_owned()]);
    assert!(!format!("{conn:?}").contains("p@ss/word"));
}

#[test]
fn refuses_the_schemes_this_store_does_not_support() {
    for bad in ["sqlite://clio.db", "sqlite:clio.db", "mysql://root@db/c"] {
        let err = parse_connection(bad).expect_err("not a postgres target");
        assert!(
            err.contains("postgres://") && err.contains("postgresql://"),
            "{bad} must name what is supported, got: {err}"
        );
    }

    // A key/value string has no scheme to check and is not a target either.
    let err = parse_connection("host=localhost port=5432").expect_err("not a url");
    assert!(err.contains("postgres://"), "{err}");
}

#[test]
fn a_malformed_url_is_refused_with_the_driver_message() {
    // Read by the driver's own parser, so the message is the driver's: these are
    // values it rejects, not ones this code inspects.
    for bad in [
        "postgres://u:p@localhost:notaport/c",
        "postgres://u:p@localhost:99999/c",
        "postgres://u:p@[::1/c",
    ] {
        let err = parse_connection(bad).expect_err("the driver refuses it");
        assert!(err.contains("database url is not usable"), "{bad}: {err}");
        // The parse refused it, so no connection was ever attempted.
        assert!(!err.contains("postgres connect"), "{bad}: {err}");
    }

    // An unknown mode is refused by this store's own reader, before the driver
    // sees the string, so the message names the modes rather than the driver's
    // bare "invalid value".
    let err =
        parse_connection("postgres://u:p@localhost/c?sslmode=maybe").expect_err("unknown mode");
    assert!(err.contains("sslmode must be"), "{err}");
}

#[test]
fn ssl_mode_names_are_read_and_the_unsupported_ones_are_refused() {
    assert_eq!(SslChoice::parse("disable").unwrap(), SslChoice::Disable);
    assert_eq!(SslChoice::parse("PREFER").unwrap(), SslChoice::Prefer);
    assert_eq!(
        SslChoice::parse(" verify-full ").unwrap(),
        SslChoice::VerifyFull
    );
    assert_eq!(
        SslChoice::parse("encrypt-no-verify").unwrap(),
        SslChoice::EncryptNoVerify
    );
    assert_eq!(SslChoice::Disable.as_str(), "disable");
    assert_eq!(SslChoice::Prefer.as_str(), "prefer");
    assert_eq!(SslChoice::VerifyFull.as_str(), "verify-full");
    assert_eq!(SslChoice::EncryptNoVerify.as_str(), "encrypt-no-verify");

    // `require` means "encrypt without checking" in other Postgres clients, so
    // its refusal must name both the verifying mode and the deliberate no-check
    // mode rather than leave the operator guessing.
    let err = SslChoice::parse("require").expect_err("require is not offered");
    assert!(err.contains("verify-full"), "{err}");
    assert!(err.contains("encrypt-no-verify"), "{err}");

    // `verify-ca` has no certificate-authority-only mode to select here, so its
    // refusal names the mode and the option that would work instead.
    let err = SslChoice::parse("verify-ca").expect_err("verify-ca is not offered");
    assert!(err.contains("verify-full"), "{err}");
    assert!(err.contains("sslrootcert"), "{err}");

    // A partial name is refused rather than resolved to a mode, so a typo cannot
    // silently select one, and `encrypt-no-verify` fails closed on a prefix.
    for bad in [
        "allow",
        "",
        "yes",
        "true",
        "required",
        "verify",
        "verify-f",
        "encrypt",
        "encrypt-no",
    ] {
        assert!(SslChoice::parse(bad).is_err(), "{bad} must be refused");
    }
}

#[test]
fn a_supplied_root_certificate_does_not_stop_the_parse() {
    // The root is read by the store and taken out before the driver's own
    // parser sees the string, so a URL that names one still parses.
    let conn = parse_connection(
        "postgres://u:p@db.example.com/c?sslmode=verify-full&sslrootcert=%2Fetc%2Fclio%2Fca.pem",
    )
    .expect("sslrootcert is read, not refused");
    assert_eq!(conn.ssl, SslChoice::VerifyFull);
    assert_eq!(conn.hosts, vec!["db.example.com".to_owned()]);
}

#[test]
fn each_url_ssl_mode_reaches_the_matching_choice() {
    // This is the mapping `connect` reads the mode through, so the whole table
    // is asserted here rather than only the default.
    for (mode, expected) in [
        ("disable", SslChoice::Disable),
        ("prefer", SslChoice::Prefer),
        ("verify-full", SslChoice::VerifyFull),
        ("encrypt-no-verify", SslChoice::EncryptNoVerify),
    ] {
        let conn =
            parse_connection(&format!("postgres://u:p@localhost/c?sslmode={mode}")).expect("parse");
        assert_eq!(conn.ssl, expected, "sslmode={mode}");
    }

    // A string that says nothing is the driver's default, which is `prefer`, and
    // so is this crate's default: a connection is encrypted when the server
    // offers it, without being refused when it does not.
    let conn = parse_connection("postgres://u:p@localhost/c").expect("parse");
    assert_eq!(conn.ssl, SslChoice::Prefer);
    assert_eq!(SslChoice::default(), SslChoice::Prefer);
}

#[test]
fn loopback_is_answered_the_same_way_everywhere() {
    for host in [
        "localhost",
        "LOCALHOST",
        "box.localhost",
        "127.0.0.1",
        "127.1.2.3",
        "::1",
        "[::1]",
    ] {
        assert!(is_loopback(host), "{host} is this machine");
    }
    for host in [
        "[fe80::1%en0]",
        "db.example.com",
        "10.0.0.1",
        "trusted@evil.example",
    ] {
        assert!(!is_loopback(host), "{host} is not this machine");
    }
}

#[test]
fn a_parsed_connection_knows_whether_it_is_local() {
    assert!(
        parse_connection("postgres://u:p@localhost/c")
            .unwrap()
            .is_local()
    );
    assert!(
        parse_connection("postgres://u:p@box.localhost:34310/c")
            .unwrap()
            .is_local()
    );
    assert!(
        parse_connection("postgres://u:p@[::1]:5432/c")
            .unwrap()
            .is_local()
    );
    assert!(
        !parse_connection("postgres://u:p@db.example.com/c")
            .unwrap()
            .is_local()
    );

    // A string that named no host is not local: the driver refuses it, and
    // claiming otherwise would put the wrong warning in front of the operator.
    let no_host = parse_connection("postgres:///clio").expect("a string with no host still parses");
    assert!(no_host.hosts.is_empty(), "{:?}", no_host.hosts);
    assert!(!no_host.is_local());
    assert_eq!(no_host.primary_host(), "");
    assert_eq!(no_host.primary_port(), 5432);
}
