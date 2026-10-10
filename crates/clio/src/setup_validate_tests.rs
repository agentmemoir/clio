// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests every format check `clio setup` runs: the addresses it
//! accepts, the ones it refuses, the endpoint rule, the SSL default by host,
//! and the routing of a failed connection test back to a field.
//!
//! ## Owns
//! - Accepted and refused addresses, with the reason in the refusal
//! - The remote plain-`http` refusal and the loopback endpoints it must keep
//! - `default_ssl_for` in both directions
//! - `Blame::of` against representative driver messages
//!
//! ## Does not own
//! - The checks themselves (see `setup_validate`)
//! - The driver's parser, which is what produces the parse messages
//!
//! ## Boundary
//!
//! Tests here break when a check is loosened or tightened; they never change
//! the implementation.
//!
//! Keep this module focused on what the wizard accepts and refuses.

use super::*;
use clio_store::SslChoice;

/// Addresses the wizard must accept unchanged.
const ACCEPTED: &[&str] = &[
    "postgres://u:p@h:5432/db",
    "postgresql://u:p@h:5432/db?sslmode=verify-full",
    "postgres://u:p@h1:5432,h2:5432/db",
    "postgres://u:a%20b%26c@h:5432/db",
    "postgres://u:p@db.example.com/memoir",
];

#[test]
fn a_plain_address_is_accepted() {
    assert!(check_connection("postgres://u:p@h:5432/db").is_ok());
}

#[test]
fn the_postgresql_spelling_is_accepted() {
    assert!(check_connection("postgresql://u:p@h:5432/db?sslmode=verify-full").is_ok());
}

#[test]
fn a_comma_separated_host_list_is_accepted() {
    assert!(check_connection("postgres://u:p@h1:5432,h2:5432/db").is_ok());
}

#[test]
fn a_percent_encoded_password_is_accepted() {
    assert!(check_connection("postgres://u:a%20b%26c@h:5432/db").is_ok());
}

#[test]
fn a_sqlite_scheme_is_refused() {
    let error = check_connection("sqlite:///data/clio.db").expect_err("not a database target");
    assert!(
        error.contains("postgres") || error.contains("sqlite"),
        "names the problem: {error}"
    );
}

#[test]
fn a_mysql_scheme_is_refused() {
    let error = check_connection("mysql://u:p@h:3306/db").expect_err("not a database target");
    assert!(
        error.contains("postgres://"),
        "names what is supported: {error}"
    );
}

#[test]
fn an_address_with_no_host_is_refused() {
    let error = check_connection("postgres:///db").expect_err("a socket default is not a host");
    assert!(error.contains("no host"), "{error}");
}

#[test]
fn verify_full_in_an_address_is_accepted() {
    assert!(check_connection("postgres://u:p@h:5432/db?sslmode=verify-full").is_ok());
}

#[test]
fn require_in_an_address_is_refused_with_the_modes_to_use_instead() {
    let error = check_connection("postgres://u:p@h:5432/db?sslmode=require")
        .expect_err("require is not offered");
    assert!(error.contains("verify-full"), "{error}");
    assert!(error.contains("encrypt-no-verify"), "{error}");
}

#[test]
fn verify_ca_in_an_address_is_refused() {
    let error = check_connection("postgres://u:p@h:5432/db?sslmode=verify-ca")
        .expect_err("this store cannot honour verify-ca");
    assert!(error.contains("verify-ca"), "{error}");
    assert!(
        error.contains("sslrootcert"),
        "names the option that would work: {error}"
    );
}

#[test]
fn every_accepted_address_passes_the_check() {
    for address in ACCEPTED {
        assert!(
            check_connection(address).is_ok(),
            "{address} must be accepted: {:?}",
            check_connection(address)
        );
    }
}

#[test]
fn a_pasted_address_must_be_a_complete_url() {
    let error = check_pasted_connection("db.example.com:5432/clio")
        .expect_err("a bare host and port is not a paste");
    assert!(error.contains("postgres://"), "{error}");
}

#[test]
fn a_pasted_complete_address_is_accepted() {
    assert!(check_pasted_connection("postgres://u:p@h:5432/db").is_ok());
}

#[test]
fn a_host_is_required() {
    assert!(check_host("").is_err());
}

#[test]
fn a_plain_host_is_accepted() {
    assert!(check_host("db.example.com").is_ok());
}

#[test]
fn a_host_list_is_accepted() {
    assert!(check_host("db1.example.com,db2.example.com").is_ok());
}

#[test]
fn a_host_with_a_scheme_is_refused() {
    let error = check_host("postgres://db.example.com").expect_err("scheme is not a host");
    assert!(error.contains("scheme"), "{error}");
}

#[test]
fn a_host_with_a_username_is_refused() {
    let error = check_host("u@db.example.com").expect_err("userinfo is not a host");
    assert!(error.contains("username"), "{error}");
}

#[test]
fn a_host_list_with_an_empty_entry_is_refused() {
    let error = check_host("db1.example.com,").expect_err("an empty entry names nothing");
    assert!(error.contains("empty entry"), "{error}");
}

#[test]
fn a_port_in_range_is_accepted() {
    assert!(check_port("5432").is_ok());
    assert!(check_port("1").is_ok());
    assert!(check_port("65535").is_ok());
}

#[test]
fn a_port_list_is_accepted() {
    assert!(check_port("5432,5433").is_ok());
}

#[test]
fn a_zero_port_is_refused() {
    assert!(check_port("0").is_err());
}

#[test]
fn a_port_above_the_range_is_refused() {
    let error = check_port("70000").expect_err("65535 is the top");
    assert!(error.contains("65535"), "{error}");
}

#[test]
fn a_non_numeric_port_is_refused() {
    assert!(check_port("five-thousand").is_err());
}

#[test]
fn a_username_is_required() {
    assert!(check_username("  ").is_err());
}

#[test]
fn a_plain_username_is_accepted() {
    assert!(check_username("clio").is_ok());
}

#[test]
fn a_username_with_a_colon_is_refused() {
    let error = check_username("cli:o").expect_err("the colon would end the username");
    assert!(error.contains("username"), "{error}");
}

#[test]
fn a_database_name_is_required() {
    assert!(check_database_name("").is_err());
}

#[test]
fn a_plain_database_name_is_accepted() {
    assert!(check_database_name("memoir_1").is_ok());
}

#[test]
fn a_database_name_with_a_slash_is_refused() {
    let error = check_database_name("memoir/other").expect_err("the slash starts a path");
    assert!(error.contains("database name"), "{error}");
}

#[test]
fn a_database_name_with_a_space_is_refused() {
    assert!(check_database_name("my memoir").is_err());
}

#[test]
fn a_loopback_http_endpoint_is_accepted() {
    assert!(check_endpoint("http://127.0.0.1:8080").is_ok());
}

#[test]
fn a_localhost_endpoint_is_accepted() {
    assert!(check_endpoint("http://localhost:8080").is_ok());
}

#[test]
fn a_localhost_subdomain_endpoint_is_accepted() {
    assert!(check_endpoint("http://box.localhost:8080").is_ok());
}

#[test]
fn a_remote_https_endpoint_is_accepted() {
    assert!(check_endpoint("https://api.example.com").is_ok());
}

#[test]
fn a_remote_plain_http_endpoint_is_refused() {
    let error = check_endpoint("http://api.example.com")
        .expect_err("the API key would cross the network in the clear");
    assert!(error.contains("https"), "{error}");
}

#[test]
fn a_non_http_scheme_is_refused() {
    assert!(check_endpoint("ftp://api.example.com").is_err());
}

#[test]
fn an_endpoint_with_no_host_is_refused() {
    let error = check_endpoint("https://").expect_err("a scheme alone is not an endpoint");
    assert!(error.contains("host") || error.contains("url"), "{error}");
}

#[test]
fn a_remote_host_defaults_to_require() {
    assert_eq!(default_ssl_for("db.example.com"), SslChoice::VerifyFull);
}

#[test]
fn a_loopback_host_defaults_to_prefer() {
    assert_eq!(default_ssl_for("127.0.0.1"), SslChoice::Prefer);
    assert_eq!(default_ssl_for("localhost"), SslChoice::Prefer);
}

#[test]
fn an_empty_host_is_treated_as_remote() {
    assert_eq!(default_ssl_for(""), SslChoice::VerifyFull);
}

#[test]
fn a_host_list_takes_its_first_entry() {
    assert_eq!(
        default_ssl_for("127.0.0.1,db.example.com"),
        SslChoice::Prefer
    );
}

#[test]
fn every_advanced_setting_accepts_an_empty_value() {
    for setting in crate::setup_db_options::ADVANCED {
        assert!(
            setting.check("").is_ok(),
            "{} must accept the driver's own default",
            setting.name
        );
    }
}

#[test]
fn target_session_attrs_refuses_primary() {
    let error = check_advanced("target_session_attrs", "primary")
        .expect_err("the driver accepts only any|read-write|read-only");
    assert!(error.contains("read-only"), "{error}");
}

#[test]
fn target_session_attrs_accepts_read_only() {
    assert!(check_advanced("target_session_attrs", "read-only").is_ok());
}

#[test]
fn channel_binding_accepts_require() {
    assert!(check_advanced("channel_binding", "require").is_ok());
}

#[test]
fn an_unknown_advanced_setting_is_refused() {
    let error = check_advanced("replication", "true").expect_err("not a connection setting");
    assert!(error.contains("replication"), "{error}");
}

#[test]
fn a_seconds_setting_refuses_a_non_number() {
    let error = check_advanced("connect_timeout", "soon").expect_err("seconds are a number");
    assert!(error.contains("seconds"), "{error}");
}

#[test]
fn a_seconds_setting_refuses_zero() {
    assert!(check_advanced("connect_timeout", "0").is_err());
}

#[test]
fn a_count_setting_accepts_zero() {
    assert!(check_advanced("keepalives_retries", "0").is_ok());
}

#[test]
fn keepalives_accepts_on_and_off() {
    assert!(check_advanced("keepalives", "on").is_ok());
    assert!(check_advanced("keepalives", "off").is_ok());
    assert!(check_advanced("keepalives", "maybe").is_err());
}

#[test]
fn the_ed25519_caveat_is_in_the_help() {
    let setting = crate::setup_db_options::advanced("channel_binding").expect("a setting");
    let caveat = setting.caveat.expect("the Ed25519 caveat is stated");
    assert!(caveat.contains("Ed25519"), "{caveat}");
}

#[test]
fn the_postgres_seventeen_caveat_is_in_the_help() {
    let setting = crate::setup_db_options::advanced("sslnegotiation").expect("a setting");
    let caveat = setting.caveat.expect("the version caveat is stated");
    assert!(caveat.contains("17"), "{caveat}");
    assert!(caveat.contains("prefer"), "{caveat}");
}

#[test]
fn a_wrong_password_is_blamed_on_the_credentials() {
    let message = "db error: FATAL: password authentication failed for user \"clio\"";
    assert_eq!(Blame::of(message), Blame::Credentials);
}

#[test]
fn an_unknown_role_is_blamed_on_the_credentials() {
    let message = "db error: FATAL: role \"clio\" does not exist";
    assert_eq!(Blame::of(message), Blame::Credentials);
}

#[test]
fn a_missing_database_is_blamed_on_the_name() {
    let message = "db error: FATAL: database \"memoir\" does not exist";
    assert_eq!(Blame::of(message), Blame::Name);
}

#[test]
fn a_dns_failure_is_blamed_on_the_host() {
    let message = "error resolving host name: could not translate host name \"db.example.com\": \
                   Name or service not known";
    assert_eq!(Blame::of(message), Blame::Host);
}

#[test]
fn a_refused_socket_is_blamed_on_the_port() {
    assert_eq!(Blame::of("connection refused (os error 61)"), Blame::Port);
}

#[test]
fn a_certificate_refusal_is_blamed_on_the_ssl_mode() {
    let message = "db error: server rejected the connection: SSL error: certificate verify failed";
    assert_eq!(Blame::of(message), Blame::Ssl);
}

/// The driver's own `Display` for a server-side refusal is the bare string
/// `db error`, with the reason in the error source below it. Attributed to
/// nothing, because it says nothing.
#[test]
fn a_bare_db_error_is_blamed_on_nothing() {
    assert_eq!(Blame::of("db error"), Blame::Everything);
}

/// Walked to the source, the same failure becomes actionable.
#[test]
fn a_db_error_with_its_source_is_blamed_on_the_field() {
    let message = "db error: FATAL: password authentication failed for user \"clio\"";
    assert_eq!(Blame::of(message), Blame::Credentials);
}

#[test]
fn an_unrecognised_message_is_blamed_on_nothing() {
    assert_eq!(
        Blame::of("the driver changed its mind about the weather"),
        Blame::Everything
    );
}
