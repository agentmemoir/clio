// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the field-level rules: which characters a username may
//! not hold, which question each blame sends the operator back to, and how an
//! SSL mode is spelled in an address.
//!
//! ## Owns
//! - The username rule at its boundary
//! - One label per `Blame`
//! - The mode names and the query fragment
//!
//! ## Does not own
//! - The checks that use the username rule (see `setup_validate`)
//! - The plan that writes the query fragment (see `setup_plan_db`)
//!
//! ## Boundary
//!
//! Tests here break when a field rule changes; they never change the
//! implementation.
//!
//! Keep this module focused on the field rules.

use super::*;
use crate::setup_validate::Blame;

#[test]
fn a_plain_username_is_acceptable() {
    assert!(username_ok("clio"));
}

#[test]
fn a_padded_username_is_acceptable() {
    assert!(username_ok("  clio  "));
}

#[test]
fn an_empty_username_is_not_acceptable() {
    assert!(!username_ok("   "));
}

#[test]
fn a_username_holding_a_colon_is_not_acceptable() {
    // The colon would end the username and start the password.
    assert!(!username_ok("cli:o"));
}

#[test]
fn a_username_holding_an_at_sign_is_not_acceptable() {
    assert!(!username_ok("cli@o"));
}

#[test]
fn a_username_holding_a_space_is_not_acceptable() {
    assert!(!username_ok("cli o"));
}

#[test]
fn a_username_holding_a_percent_is_not_acceptable() {
    // A typed percent would be read as the start of an escape.
    assert!(!username_ok("cli%20o"));
}

#[test]
fn a_host_failure_names_the_host_question() {
    assert_eq!(Blame::Host.label(), "Host");
}

#[test]
fn a_port_failure_names_the_port_question() {
    assert_eq!(Blame::Port.label(), "Port");
}

#[test]
fn a_credentials_failure_names_the_credentials_question() {
    assert_eq!(Blame::Credentials.label(), "Username or password");
}

#[test]
fn a_name_failure_names_the_database_name_question() {
    assert_eq!(Blame::Name.label(), "Database name");
}

#[test]
fn an_ssl_failure_names_the_ssl_question() {
    assert_eq!(Blame::Ssl.label(), "SSL mode");
}

/// `Everything` gets no field, because the driver attributed the failure to
/// nothing in particular and there is no one answer to change.
#[test]
fn an_unattributed_failure_names_no_field() {
    assert_eq!(Blame::Everything.label(), "the database connection");
}

#[test]
fn every_blame_has_a_distinct_label() {
    let mut sorted = vec![
        Blame::Host.label(),
        Blame::Port.label(),
        Blame::Credentials.label(),
        Blame::Name.label(),
        Blame::Ssl.label(),
        Blame::Everything.label(),
    ];
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 6, "two fields share a label: {sorted:?}");
}

#[test]
fn there_are_exactly_four_ssl_mode_names() {
    assert_eq!(
        ssl_mode_names(),
        ["disable", "prefer", "verify-full", "encrypt-no-verify"]
    );
}

#[test]
fn an_ssl_mode_is_spelled_as_a_query_parameter() {
    assert_eq!(ssl_query(SslChoice::VerifyFull), "sslmode=verify-full");
    assert_eq!(ssl_query(SslChoice::Prefer), "sslmode=prefer");
    assert_eq!(ssl_query(SslChoice::Disable), "sslmode=disable");
    assert_eq!(
        ssl_query(SslChoice::EncryptNoVerify),
        "sslmode=encrypt-no-verify"
    );
}

#[test]
fn the_ssl_parameter_name_is_the_one_the_driver_reads() {
    assert_eq!(SSL_PARAM, "sslmode");
}

#[test]
fn the_root_parameter_name_is_the_one_libpq_uses() {
    assert_eq!(SSL_ROOT_PARAM, "sslrootcert");
}
