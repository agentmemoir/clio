// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the ways the database group ends
//! without a connection: an answer that names no route, a flag the driver would
//! refuse, a run with no terminal to ask, and the waiver that records an untested
//! address instead of stopping.
//!
//! ## Owns
//! - The two choice questions' refusals and defaults under test for
//!   `setup_flows_db`
//! - The per-field flag refusals the managed route produces
//! - The `--allow-untested` path out of a refused address
//!
//! ## Does not own
//! - The routes that reach a plan (see `setup_flows_db_tests`)
//! - The field checks themselves (see `setup_validate_tests`)
//! - The typed route's thirteen advanced settings (see `setup_flows_db_tests`)
//!
//! ## Boundary
//!
//! Changes to what the database group refuses should normally update tests here.
//! Changes to what a completed route writes should not require changes here.
//!
//! Keep this module focused on the paths that end a run early.

// Every flag that answers the managed route outright, so a test about one field's
// refusal does not have to answer the other three by keyboard. There is no
// password flag here: the managed route has no password, and passing one is a
// refusal in its own right — see `the_managed_route_refuses_a_password_flag` in
// the settings suite.
const MANAGED_FIELDS: [(&str, &str); 4] = [
    ("database-host", "db.example.com"),
    ("database-port", "5432"),
    ("database-user", "clio"),
    ("database-name", "clio"),
];

/// The three service questions answered by flag, so a run that reaches them by
/// keyboard is not what these tests are measuring.
const SERVICES_BY_FLAG: [(&str, &str); 3] =
    [("embed", "local"), ("rerank", "skip"), ("extract", "skip")];

/// An address a paste route can be given, with the mode spelled out so it does
/// not depend on the host-derived default.
const ADDRESS: &str = "postgres://clio:pw@db.example.com:5432/clio?sslmode=prefer";

use crate::setup_plan_db::DatabaseChoice;
use crate::setup_preview_fixtures::{Harness, SERVICES};

fn choice_of(run: &crate::setup_preview_fixtures::Run) -> DatabaseChoice {
    run.answers()
        .database
        .as_ref()
        .expect("a database plan")
        .plan
        .choice
}

#[test]
fn an_answer_that_names_no_option_is_asked_again_rather_than_guessed() {
    // The three-way question is a closed set, so a value outside it is a typo. It
    // is re-asked rather than read as the default, because falling back to a route
    // the operator did not choose would install somewhere they did not ask for.
    let run = Harness::fresh()
        .typing("9\n1\ndb.example.com\n\n\n\n\n")
        .typing(SERVICES)
        .run();
    assert_eq!(choice_of(&run), DatabaseChoice::Managed, "{}", run.out);
    assert!(
        run.out
            .contains("`9` is not one of: 1 managed, 2 sqlite, 3 existing"),
        "the re-ask lists what there is to choose: {}",
        run.out
    );
}

#[test]
fn enter_at_the_database_question_keeps_the_managed_default() {
    // Enter is the reflex answer and the question's default is the fresh-install
    // route, so a blank line has to reach the managed route rather than fail.
    let run = Harness::fresh()
        .typing("\ndb.example.com\n\n\n\n\n")
        .typing(SERVICES)
        .run();
    assert_eq!(choice_of(&run), DatabaseChoice::Managed, "{}", run.out);
}

#[test]
fn a_non_interactive_run_takes_the_managed_default_without_asking() {
    // No terminal means no question, so the run has to take the same default the
    // interactive question would have offered rather than refusing for want of an
    // answer it was never going to get.
    let run = Harness::fresh()
        .non_interactive()
        .with_secrets(&["pw"])
        .flags(MANAGED_FIELDS)
        .flags(SERVICES_BY_FLAG)
        .run();
    assert_eq!(choice_of(&run), DatabaseChoice::Managed, "{}", run.out);
}

#[test]
fn an_answer_that_names_no_route_is_asked_again_rather_than_guessed() {
    // The same rule one question down. Guessing here would be worse than at the
    // three-way question, because the two routes are not equally private: a paste
    // is left in the terminal's scrollback and a typed field is not.
    let run = Harness::fresh()
        .with_secrets(&[ADDRESS])
        .typing("3\n9\n1\n")
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.address, ADDRESS, "the re-ask took the pasted route");
    assert!(
        run.out.contains("`9` is not one of: 1 pasted, 2 typed"),
        "the re-ask lists the two routes: {}",
        run.out
    );
}

#[test]
fn enter_at_the_hand_over_question_keeps_the_pasted_default() {
    // The sub-choice's default is the paste route. Enter must take it, because a
    // blank line at a question is an answer to that question, not a request to
    // ask it again.
    let run = Harness::fresh()
        .with_secrets(&[ADDRESS])
        .typing("3\n\n")
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.address, ADDRESS, "the paste route is the default");
}

#[test]
fn quitting_at_the_hand_over_question_stops_the_run() {
    // `q` is a way out of every question, and the two routes below it are the only
    // ones that would otherwise re-ask it forever.
    let run = Harness::fresh().typing("3\nq\n").drive();
    assert!(run.outcome.is_err(), "q must stop the run: {}", run.out);
    assert!(
        !run.out.contains("Connection string"),
        "q must not go on to ask for the address: {}",
        run.out
    );
}

#[test]
fn a_non_interactive_existing_run_takes_the_pasted_default() {
    // The route is named by flag but the hand-over is not, so the sub-choice has
    // no answer to read and has to fall back to the same default the interactive
    // question offers.
    let run = Harness::fresh()
        .non_interactive()
        .flags([
            ("database", "existing"),
            ("database-url", ADDRESS),
            ("database-password", "pw"),
        ])
        .flags(SERVICES_BY_FLAG)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.address, ADDRESS, "{}", run.out);
}

#[test]
fn allow_untested_records_a_refused_address_instead_of_stopping() {
    // The waiver is the only way a run reaches the preview with an address nobody
    // has connected to, so the plan has to record that it was never tested rather
    // than report a successful test.
    let run = Harness::fresh()
        .database_fails("connection refused")
        .flag("allow-untested", "true")
        .with_secrets(&[ADDRESS])
        .typing("3\n1\n")
        .typing(SERVICES)
        .run();
    let database = run.answers().database.as_ref().expect("a plan");
    assert_eq!(
        database.tested, None,
        "a waived address is an untested one, not a tested one"
    );
    assert!(
        run.out
            .contains("--allow-untested saved the answers anyway"),
        "{}",
        run.out
    );
}

// One refusal per field check, on the managed route.
//
// The fields are answered by flag, because a field answered by keyboard is
// re-asked until it is valid and never reaches the refusal at all. A flag is the
// operator speaking once, so a flag the driver would refuse ends the run at the
// question that named it rather than silently dropping the setting.
#[test]
fn a_host_flag_the_driver_would_refuse_is_refused() {
    let reason = Harness::fresh()
        .non_interactive()
        .flag("database", "managed")
        .flags(MANAGED_FIELDS)
        .flag("database-host", "db.example.com,")
        .expect_refusal();
    assert!(reason.contains("--database-host"), "{reason}");
    assert!(reason.contains("empty entry"), "{reason}");
}

#[test]
fn a_port_flag_that_is_not_a_number_is_refused() {
    let reason = Harness::fresh()
        .non_interactive()
        .flag("database", "managed")
        .flags(MANAGED_FIELDS)
        .flag("database-port", "not-a-port")
        .expect_refusal();
    assert!(reason.contains("--database-port"), "{reason}");
    assert!(reason.contains("whole number"), "{reason}");
}

#[test]
fn a_username_flag_that_would_end_the_address_is_refused() {
    let reason = Harness::fresh()
        .non_interactive()
        .flag("database", "managed")
        .flags(MANAGED_FIELDS)
        .flag("database-user", "bad user")
        .expect_refusal();
    assert!(reason.contains("--database-user"), "{reason}");
    assert!(reason.contains("cannot be used"), "{reason}");
}

#[test]
fn a_database_name_flag_with_a_path_separator_is_refused() {
    let reason = Harness::fresh()
        .non_interactive()
        .flag("database", "managed")
        .flags(MANAGED_FIELDS)
        .flag("database-name", "bad/name")
        .expect_refusal();
    assert!(reason.contains("--database-name"), "{reason}");
    assert!(reason.contains("cannot hold"), "{reason}");
}

#[test]
fn the_managed_route_refuses_the_no_verify_mode() {
    // The deliberate no-check mode is for a server whose certificate Clio did
    // not issue. A container Clio started has no certificate, so the mode is
    // refused by name rather than accepted and failed at the connection test.
    let reason = Harness::fresh()
        .flag("database-ssl", "encrypt-no-verify")
        .typing(crate::setup_preview_fixtures::MANAGED)
        .typing(SERVICES)
        .expect_refusal();
    assert!(reason.contains("encrypt-no-verify"), "{reason}");
    assert!(reason.contains("managed"), "{reason}");
}

#[test]
fn the_managed_route_refuses_a_root_certificate_flag() {
    // The root certificate belongs to a server Clio did not issue. The managed
    // route reaches no such server, so the flag is refused by name rather than
    // accepted and never used.
    let reason = Harness::fresh()
        .flag("database-ssl-rootcert", "/etc/ca.pem")
        .typing(crate::setup_preview_fixtures::MANAGED)
        .typing(SERVICES)
        .expect_refusal();
    assert!(reason.contains("--database-ssl-rootcert"), "{reason}");
}

#[test]
fn the_sqlite_route_refuses_a_root_certificate_flag() {
    let reason = Harness::fresh()
        .flag("database-ssl-rootcert", "/etc/ca.pem")
        .typing(crate::setup_preview_fixtures::SQLITE)
        .typing(SERVICES)
        .expect_refusal();
    assert!(reason.contains("--database-ssl-rootcert"), "{reason}");
}

#[test]
fn the_paste_route_refuses_a_root_certificate_flag() {
    // A pasted address is kept as given and carries its own `sslrootcert`, so
    // the flag cannot reach it. It is refused rather than silently dropped.
    let reason = Harness::fresh()
        .flag("database-ssl-rootcert", "/etc/ca.pem")
        .with_secrets(&[ADDRESS])
        .typing("3\n1\n")
        .typing(SERVICES)
        .expect_refusal();
    assert!(reason.contains("--database-ssl-rootcert"), "{reason}");
}

#[test]
fn the_paste_route_refuses_an_advanced_option_flag() {
    // A pasted address is kept as given and carries its own query string, so
    // `--database-option` cannot reach it. The top-level check only refuses the
    // flag on the routes that are not `existing`, so the pasted route has to
    // refuse it here or the run reports success with the setting silently gone.
    // The flags mirror the reported command, where the pasted route is named
    // outright rather than chosen at the hand-over question.
    let reason = Harness::fresh()
        .non_interactive()
        .flags([
            ("database", "existing"),
            ("database-connect", "pasted"),
            ("database-url", ADDRESS),
            ("database-option", "application_name=clio"),
        ])
        .flags(SERVICES_BY_FLAG)
        .expect_refusal();
    assert!(reason.contains("--database-option"), "{reason}");
    assert!(reason.contains("typed route"), "{reason}");
}
