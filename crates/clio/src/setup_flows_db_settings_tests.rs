// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the typed route's connection
//! settings: the defaults the block feeds, the thirteen advanced settings, and
//! the repeatable `--database-option` flag.
//!
//! ## Owns
//! - The typed route's field defaults and advanced block under test for
//!   `setup_flows_db`
//!
//! ## Does not own
//! - The routes that reach a plan (see `setup_flows_db_tests`)
//! - The refusals and the attempt cap (see `setup_flows_db_refusal_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to the fields a connection is built from should normally update tests
//! here. Changes to which route is chosen should not require changes here.
//!
//! Keep this module focused on the settings inside one connection string.

use crate::setup_preview_fixtures::{Harness, SERVICES, TYPED};
use serde_json::json;

#[test]
fn a_remote_host_defaults_to_verify_full_and_this_machine_to_prefer() {
    let remote = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let local = Harness::fresh()
        .with_secrets(&["pw"])
        .typing("3\n2\n127.0.0.1\n5432\nclio\n\n\nn\n")
        .typing(SERVICES)
        .run();
    let ssl_of = |run: &crate::setup_preview_fixtures::Run| {
        run.answers()
            .database
            .as_ref()
            .expect("a plan")
            .plan
            .ssl
            .as_str()
            .to_owned()
    };
    assert_eq!(ssl_of(&remote), "verify-full", "{}", remote.out);
    assert_eq!(ssl_of(&local), "prefer", "{}", local.out);
}

#[test]
fn choosing_encrypt_no_verify_warns_at_the_choice_and_is_recorded() {
    // Option 4 at the SSL question is the deliberate no-check mode. The run must
    // say, at the point of choice, what it does and does not protect against, and
    // the plan must carry the mode so status can report it afterwards.
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing("3\n2\ndb.example.com\n5432\nclio\n\n4\n")
        .typing(SERVICES)
        .run();
    assert!(
        run.out.contains("does not check the server certificate"),
        "the warning is printed at the choice: {}",
        run.out
    );
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.ssl.as_str(), "encrypt-no-verify");
}

#[test]
fn the_database_name_defaults_to_the_username() {
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert!(plan.address.contains("/clio?"), "{}", plan.address);
}

#[test]
fn the_advanced_block_is_off_by_default() {
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert!(plan.extra.is_empty(), "{:?}", plan.extra);
    assert!(
        !plan.address.contains("application_name"),
        "{}",
        plan.address
    );
}

#[test]
fn a_typed_run_asks_no_advanced_questions() {
    // The block was thirteen questions between the operator and a working
    // install, and everything it collected is reachable by flag. This pins the
    // absence: the typed route is the six fields and the connection test, and a
    // block that came back would put thirteen questions back in front of
    // everyone to serve the rare operator who wants them.
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert!(
        plan.extra.is_empty(),
        "no block was asked: {:?}",
        plan.extra
    );
    assert!(
        !run.out.contains("target_session_attrs"),
        "the block's own field names must not appear: {}",
        run.out
    );
}

#[test]
fn an_advanced_setting_the_flag_names_is_still_written() {
    // What the block collected is reachable by flag, and the flag is checked
    // against the same table the block used, so a value the driver would refuse
    // is refused here too — before anything is written, not after.
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .flags([
            ("database", "existing"),
            ("database-connect", "typed"),
            ("database-host", "db.example.com"),
            ("database-port", "5432"),
            ("database-name", "clio"),
            ("database-user", "clio"),
            ("database-password", "pw"),
            ("database-ssl", "prefer"),
            ("database-option", "options=-c%20k1"),
        ])
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.extra.len(), 1, "{:?}", plan.extra);
    assert_eq!(plan.extra[0].name, "options");
    // The flag's value is already percent-encoded, and the address encodes it once
    // more — so the stored address carries `%2520` where the flag carried `%20`.
    // Asserted on the double-encoded form because that is what the driver reads
    // back, and a single round of encoding here would hand Postgres a value with a
    // literal `%20` in it.
    assert!(
        plan.address.contains("options=-c%2520k1"),
        "{}",
        plan.address
    );
}

#[test]
fn an_advanced_setting_the_driver_would_refuse_is_refused_by_flag() {
    // The same table, the same refusal, without the thirteen questions. An
    // operator who reaches for a flag is an operator who can be told why.
    let reason = Harness::fresh()
        .flags([
            ("database", "existing"),
            ("database-connect", "typed"),
            ("database-host", "db.example.com"),
            ("database-port", "5432"),
            ("database-name", "clio"),
            ("database-user", "clio"),
            ("database-password", "pw"),
            ("database-ssl", "prefer"),
            ("database-option", "target_session_attrs=sideways"),
        ])
        .typing(SERVICES)
        .expect_refusal();
    assert!(reason.contains("target_session_attrs"), "{reason}");
}

#[test]
fn a_repeatable_option_flag_appends_rather_than_replacing() {
    let run = Harness::fresh()
        .flags([
            ("database", "existing"),
            ("database-connect", "typed"),
            ("database-host", "db.example.com"),
            ("database-port", "5432"),
            ("database-name", "clio"),
            ("database-user", "clio"),
            ("database-password", "pw"),
            ("database-ssl", "prefer"),
            ("database-option", "options=-c%20k1\napplication_name=clio"),
        ])
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.extra.len(), 2, "{:?}", plan.extra);
    assert!(
        plan.address.contains("application_name=clio"),
        "{}",
        plan.address
    );
}

#[test]
fn a_managed_run_writes_an_address_with_no_password() {
    // The managed route asks nothing about a password any more, so a stored
    // address that used to carry one changes nothing: the new plan is
    // passwordless whatever the config held before. That is what stops a later
    // run from invalidating a database that is already working.
    let run = Harness::fresh()
        .with_effective(json!({
            "store": { "database_url": "postgres://clio:stored-value@127.0.0.1:5432/clio?sslmode=prefer" }
        }))
        .typing(crate::setup_preview_fixtures::MANAGED)
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert!(
        !plan.address.contains("stored-value"),
        "the managed address carries no password: {}",
        plan.address
    );
    assert!(
        !plan.address.contains("stored-value"),
        "the stored password does not survive into the new address: {}",
        plan.address
    );
    assert!(
        plan.address.starts_with("postgres://clio@"),
        "{}",
        plan.address
    );
}

#[test]
fn a_root_certificate_with_a_verifying_mode_is_written() {
    // The typed route's root certificate is a flag, and a verifying mode is the
    // only one that uses it. The flag reaches the address, percent-encoded the
    // way the rest of the value is.
    let run = Harness::fresh()
        .flags([
            ("database", "existing"),
            ("database-connect", "typed"),
            ("database-host", "db.example.com"),
            ("database-port", "5432"),
            ("database-name", "clio"),
            ("database-user", "clio"),
            ("database-password", "pw"),
            ("database-ssl", "verify-full"),
            ("database-ssl-rootcert", "/etc/ca.pem"),
        ])
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert!(
        plan.address.contains("sslrootcert=%2Fetc%2Fca.pem"),
        "{}",
        plan.address
    );
}

#[test]
fn a_root_certificate_with_a_non_verifying_mode_is_refused() {
    // A mode that does not check the certificate cannot use a root, so the flag
    // is refused by name rather than carried into an address that ignores it.
    let reason = Harness::fresh()
        .flags([
            ("database", "existing"),
            ("database-connect", "typed"),
            ("database-host", "db.example.com"),
            ("database-port", "5432"),
            ("database-name", "clio"),
            ("database-user", "clio"),
            ("database-password", "pw"),
            ("database-ssl", "disable"),
            ("database-ssl-rootcert", "/etc/ca.pem"),
        ])
        .typing(SERVICES)
        .expect_refusal();
    assert!(reason.contains("--database-ssl-rootcert"), "{reason}");
    assert!(reason.contains("checks the certificate"), "{reason}");
}

#[test]
fn the_managed_route_refuses_a_password_flag() {
    let reason = Harness::fresh()
        .flag("database-password", "settings-password-0123456789")
        .typing(crate::setup_preview_fixtures::MANAGED)
        .typing(SERVICES)
        .expect_refusal();
    assert!(
        reason.contains("--database-password"),
        "the refusal has to name the flag: {reason}"
    );
    assert!(
        reason.contains("managed"),
        "the refusal has to name the route it does not apply to: {reason}"
    );
}
