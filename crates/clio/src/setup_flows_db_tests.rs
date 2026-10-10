// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the database questions: the three
//! routes, the two hand-over routes, the blame step-back, and the attempt cap.
//!
//! ## Owns
//! - Database-flow behavior under test for `setup_flows_db` and
//!   `setup_flows_db_typed`
//!
//! ## Does not own
//! - The field defaults and the advanced settings block (see
//!   `setup_flows_db_settings_tests`)
//! - Navigation (see `setup_steps_tests`)
//! - The preview (see `setup_review_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to which database questions are asked should normally update tests
//! here. Changes to navigation should not require changes here.
//!
//! Keep this module focused on the database group.

use crate::setup_preview_fixtures::{Harness, MANAGED, SERVICES, SQLITE, TYPED, TYPED_BY_FLAG};
use crate::setup_steps::Answers;
use serde_json::Value;

fn entries_of(answers: &Answers) -> Vec<(String, Value)> {
    crate::setup_preview::planned_entries(answers)
}

#[test]
fn the_managed_route_writes_the_backend_and_the_address_together() {
    let run = Harness::fresh().typing(MANAGED).typing(SERVICES).run();
    let entries = entries_of(run.answers());
    let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths[..2], ["backend", "store.database_url"]);
    assert_eq!(entries[0].1, serde_json::json!("postgres"));
    // The host, the port, the user, and the name are all Clio's to pick on this
    // route, so the address is asserted on the shape of the choice rather than on
    // a value. The port in particular is picked at run time from the range that is
    // actually free, so pinning it would fail on any machine whose preferred port is
    // taken — and the host is this machine's loopback either way.
    //
    // The userinfo is the user name alone: the managed database has no password.
    let address = entries[1].1.as_str().expect("an address");
    assert!(
        address.starts_with("postgres://clio@127.0.0.1:"),
        "a managed address names a host and a port Clio picked, with no password: {address}"
    );
    assert!(
        address.contains("/clio?") && address.contains("sslmode="),
        "the name and the SSL mode travel with the address: {address}"
    );
    // A managed run starts the database container, and only it for local models.
    assert!(run.view().will_start.contains(&"postgres"), "{}", run.out);
}

#[test]
fn the_sqlite_route_writes_the_file_as_the_address_and_names_it_separately() {
    // `store.database_url` is the address for both backends — for SQLite the
    // address is the file — so it is written either way. `store.sqlite_path` is
    // the second copy, and it exists so switching back to Postgres has a path to
    // remove rather than leaving a stale one behind.
    let run = Harness::fresh().typing(SQLITE).typing(SERVICES).run();
    let entries = entries_of(run.answers());
    let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths[..3],
        ["backend", "store.database_url", "store.sqlite_path"],
        "{entries:?}"
    );
    assert_eq!(entries[0].1, serde_json::json!("sqlite"));
    assert_eq!(entries[1].1, entries[2].1, "the two copies must agree");
    // A file needs no container at all. The local model sidecars are a separate
    // decision this run made, so the invariant is about the database and not
    // about the whole list.
    assert!(
        !run.view().will_start.contains(&"postgres"),
        "a SQLite install must not start a database container: {:?}",
        run.view().will_start
    );
}
#[test]
fn both_hand_over_routes_reach_the_same_entries_for_the_same_server() {
    // The typed route is answered by flag, with `prefer` spelled out so it agrees
    // with the pasted string rather than defaulting to `require` for a remote
    // host. Same server, same mode, same config.
    let pasted = Harness::fresh()
        .with_secrets(&["postgres://clio:pw@db.example.com:5432/clio?sslmode=prefer"])
        .typing("3\n1\n")
        .typing(SERVICES)
        .run();
    let typed = Harness::fresh()
        .flags(TYPED_BY_FLAG)
        .typing(format!("n\n{SERVICES}"))
        .run();
    let database_of = |run: &crate::setup_preview_fixtures::Run| {
        entries_of(run.answers())
            .into_iter()
            .filter(|(path, _)| path.starts_with("backend") || path.starts_with("store."))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        database_of(&pasted),
        database_of(&typed),
        "the two routes must not produce different configs for the same server\n--- pasted ---\n{}\n--- typed ---\n{}",
        pasted.out,
        typed.out
    );
}

#[test]
fn the_typed_password_never_reaches_the_output() {
    let secret = "correct horse battery staple";
    let run = Harness::fresh()
        .with_secrets(&[secret])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    assert!(
        !run.out.contains(secret),
        "the password leaked: {}",
        run.out
    );
    assert!(
        !run.err.contains(secret),
        "the password leaked: {}",
        run.err
    );
    assert!(run.out.contains("Password:"), "{}", run.out);
}
#[test]
fn a_failed_test_returns_to_the_blamed_field_and_says_which_one() {
    // The route is answered by flag, so the re-ask has nothing to ask and the test
    // is about the message and the single retry rather than about a rewind. The
    // probe fails once, so the second attempt is the success case.
    let run = Harness::fresh()
        .database_fails_times(1, "password authentication failed for user \"clio\"")
        .flags(TYPED_BY_FLAG)
        .typing(SERVICES)
        .run();
    assert!(
        run.out.contains("The server blamed Username or password."),
        "{}",
        run.out
    );
    assert!(
        run.out.contains("That question will be asked again"),
        "{}",
        run.out
    );
    assert_eq!(
        run.out.matches("Testing the connection").count(),
        2,
        "one failure and one retry, no more: {}",
        run.out
    );
}

#[test]
fn a_credentials_failure_re_asks_the_username_as_well_as_the_password() {
    // A server that refuses a login does not say which half was wrong and cannot,
    // so the wizard asks for both. Re-asking only the password would send an
    // operator whose username was the typo to fix the one thing that was right.
    let run = Harness::fresh()
        .database_fails_times(1, "password authentication failed for user \"clio\"")
        .with_secrets(&["pw", "pw"])
        .typing(TYPED)
        .typing("clio\n\n")
        .typing(SERVICES)
        .run();
    assert!(
        run.out.contains("The server blamed Username or password."),
        "{}",
        run.out
    );
    // Counted on the prompt, not on the word, so the blame message that names
    // the field does not make a re-ask look like it happened.
    assert_eq!(
        run.out.matches("Username: ").count(),
        2,
        "the username must be asked again, not kept: {}",
        run.out
    );
}

#[test]
fn a_failed_test_keeps_the_fields_below_the_blamed_one_as_defaults() {
    // The port is blamed, so the host, the username, and the name keep their
    // answers and the run finishes on the second attempt with a new port. The
    // route is answered by flag precisely so the re-ask needs no input here.
    let run = Harness::fresh()
        .database_fails_times(1, "connection refused")
        .flags(TYPED_BY_FLAG)
        .flag("database-port", "5555")
        // The advanced question is still asked, because this is an interactive
        // run with the fields answered by flag rather than by keyboard.
        .typing(format!("n\n{SERVICES}"))
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert!(
        plan.address.contains("@db.example.com:5555"),
        "the address should carry the new port: {}",
        plan.address
    );
    assert!(run.out.contains("The server blamed Port."), "{}", run.out);
}

#[test]
fn three_failures_produce_the_refusal_and_name_the_field() {
    let reason = Harness::fresh()
        .database_fails("name or service not known")
        // The advanced block is still asked because this is an interactive run, so
        // its question is answered and the three service questions follow it.
        .flags(TYPED_BY_FLAG)
        .typing(format!("n\n{SERVICES}"))
        .expect_refusal();
    assert!(reason.contains("the connection failed 3 times"), "{reason}");
    assert!(reason.contains("blaming Host"), "{reason}");
    assert!(reason.contains("nothing was written"), "{reason}");
}
#[test]
fn a_managed_run_asks_nothing_about_a_password() {
    // The script is six lines because the managed route has no password
    // question at all — there is no password to ask about. A script with a
    // seventh would be a different wizard.
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(MANAGED)
        .typing(SERVICES)
        .run();
    assert!(!run.out.contains("Password:"), "{}", run.out);
}
