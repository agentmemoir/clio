// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the pasted-address route: that a
//! pasted address is kept exactly as given, that it is not echoed, and that the
//! hand-over's way out returns to the database choice rather than leaving.
//!
//! ## Owns
//! - Pasted-route behavior under test for `setup_flows_db_paste`
//!
//! ## Does not own
//! - The managed and SQLite routes (see `setup_flows_db_tests`)
//! - The typed route's fields and the attempt cap (see `setup_flows_db_tests`)
//! - The field defaults and the advanced settings block (see
//!   `setup_flows_db_settings_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to how a pasted address is read should normally update tests here.
//! Changes to the typed route should not require changes here.
//!
//! Keep this module focused on the pasted address.

use crate::setup_plan_db::DatabaseChoice;
use crate::setup_preview_fixtures::{Harness, SERVICES};

#[test]
fn the_paste_route_keeps_the_address_exactly_as_given() {
    let address = "postgres://clio:pw@db.example.com:5432/clio?sslmode=require&application_name=me";
    let run = Harness::fresh()
        .with_secrets(&[address])
        .typing("3\n1\n")
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.address, address, "a paste is never rebuilt");
    assert_eq!(plan.ssl.as_str(), "require", "the pasted mode is kept");
}

#[test]
fn the_way_out_at_the_hand_over_question_returns_to_the_database_choice() {
    // The hand-over question offers paste, type, and the way out, so `3` is the
    // way out and the operator lands back on the database choice rather than out
    // of the wizard. Choosing the typed route there ends the run on a typed
    // address, which is what the assertions are about.
    //
    // The pasted address is no longer part of this test's path. It carries a
    // password, so it is read through the masked reader and a reader cannot
    // produce a cancel — which is also why there is no longer a way back *from*
    // the address field to assert. See the type below for what the address is
    // read through.
    // The hand-over is inside the database section, so the way out re-asks the
    // question in front of it — the database choice — rather than leaving. So the
    // script answers the choice, backs out, answers the choice again, and this time
    // takes the typed route. The re-ask is what the test is about, and a route
    // chosen after it is what proves the run carried on rather than ending.
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing("3\n3\n3\n2\ndb.example.com\n5432\nclio\n\n\n")
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.choice, DatabaseChoice::Existing);
    assert!(
        plan.address.contains("db.example.com:5432"),
        "{}",
        plan.address
    );
    // The way out was taken once and the run carried on, so the transcript shows
    // the hand-over twice: once before the operator left it and once after they
    // came back and chose the typed route.
    assert_eq!(
        run.out
            .matches("How would you like to give me the connection?")
            .count(),
        2,
        "{}",
        run.out
    );
}

#[test]
fn a_pasted_address_that_never_parses_is_refused_after_the_attempt_cap() {
    let reason = Harness::fresh()
        .with_secrets(&["not-a-url", "not-a-url", "not-a-url"])
        .typing("3\n1\n")
        .typing(SERVICES)
        .expect_refusal();
    assert!(reason.contains("was refused 3 times"), "{reason}");
    assert!(reason.contains("nothing was written"), "{reason}");
}

#[test]
fn the_paste_route_does_not_echo_the_address_it_reads() {
    // A pasted connection string carries its password in the userinfo part, so it
    // is read through the masked reader: the field is drawn masked, and the value
    // never reaches the writer's bytes. This replaces an older test that asserted
    // the wizard *warned* about scrollback, which was true while the address was
    // typed in the clear and is not the design any more — the string is not shown
    // at all now, so there is nothing to warn about.
    let address = "postgres://clio:pw@db.example.com:5432/clio";
    let run = Harness::fresh()
        .with_secrets(&[address])
        .typing("3\n1\n")
        .typing(SERVICES)
        .run();
    assert!(run.out.contains("Connection string:"), "{}", run.out);
    assert!(
        !run.out.contains(address),
        "the address leaked: {}",
        run.out
    );
    assert!(!run.out.contains("pw@"), "the password leaked: {}", run.out);
    // The stored address is the one that was read, so the run did use it.
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.address, address, "{}", run.out);
}
