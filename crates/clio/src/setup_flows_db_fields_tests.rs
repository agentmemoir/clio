// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the two field helpers both
//! existing-Postgres routes share: the flag-or-question reader and the SSL
//! question whose default follows the host.
//!
//! ## Owns
//! - The SSL default matrix: the host-derived value and the stored value, on
//!   this machine and off it
//! - The flag, question, and non-interactive paths of `flagged_or_asked`
//!
//! ## Does not own
//! - The routes that call these helpers (see `setup_flows_db_tests`)
//! - What a valid value is (see `setup_validate_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! A change to what a shared field defaults to should normally update tests
//! here. A change to one route's questions should not require changes here.
//!
//! Keep this module focused on the helpers, not on a whole run.

use super::{ask_ssl, flagged_or_asked};
use crate::setup_defaults::Defaults;
use crate::setup_preview_fixtures::{Harness, SERVICES, TYPED, with_ctx};
use crate::setup_render::Signal;
use clio_store::SslChoice;
use serde_json::json;
use std::collections::HashMap;

/// One stored SSL mode, for a run whose config already names a database.
fn stored(mode: &str) -> Defaults {
    let mut defaults = Defaults::empty();
    defaults.ssl = Some(mode.to_owned());
    defaults
}

/// No flags.
fn no_flags() -> HashMap<String, String> {
    HashMap::new()
}

#[test]
fn a_stored_prefer_does_not_override_a_remote_host() {
    // A `prefer` written for a local server must not reach a remote host: the
    // address written now would otherwise allow plain text across the network.
    let run = Harness::fresh()
        .with_effective(json!({
            "store": { "database_url": "postgres://clio:pw@127.0.0.1:5432/clio?sslmode=prefer" }
        }))
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.ssl.as_str(), "verify-full", "{}", run.out);
}

#[test]
fn a_first_run_off_this_machine_defaults_to_verify_full() {
    let (answer, _) = with_ctx("\n", no_flags(), true, Defaults::empty(), |ctx| {
        ask_ssl(ctx, "db.example.com")
    });
    assert_eq!(answer, Ok(SslChoice::VerifyFull));
}

#[test]
fn a_first_run_on_this_machine_defaults_to_prefer() {
    let (answer, _) = with_ctx("\n", no_flags(), true, Defaults::empty(), |ctx| {
        ask_ssl(ctx, "127.0.0.1")
    });
    assert_eq!(answer, Ok(SslChoice::Prefer));
}

#[test]
fn a_stored_mode_is_kept_on_this_machine() {
    // The last run's answer for this machine is the best default there is: it
    // was chosen for the same local server.
    let (answer, _) = with_ctx("\n", no_flags(), true, stored("disable"), |ctx| {
        ask_ssl(ctx, "127.0.0.1")
    });
    assert_eq!(answer, Ok(SslChoice::Disable));
}

#[test]
fn a_stored_mode_gives_way_to_the_remote_default() {
    let (answer, _) = with_ctx("\n", no_flags(), true, stored("disable"), |ctx| {
        ask_ssl(ctx, "db.example.com")
    });
    assert_eq!(answer, Ok(SslChoice::VerifyFull));
}

#[test]
fn the_ssl_flag_beats_both_defaults() {
    let flags = HashMap::from([("database-ssl".to_owned(), "disable".to_owned())]);
    let (answer, _) = with_ctx("\n", flags, true, stored("verify-full"), |ctx| {
        ask_ssl(ctx, "db.example.com")
    });
    assert_eq!(answer, Ok(SslChoice::Disable));
}

#[test]
fn a_non_interactive_run_takes_the_derived_mode() {
    let (answer, _) = with_ctx("", no_flags(), false, Defaults::empty(), |ctx| {
        ask_ssl(ctx, "db.example.com")
    });
    assert_eq!(answer, Ok(SslChoice::VerifyFull));
}

#[test]
fn the_no_check_mode_is_recorded_and_explained_at_the_choice() {
    let (answer, printed) = with_ctx("4\n", no_flags(), true, Defaults::empty(), |ctx| {
        ask_ssl(ctx, "db.example.com")
    });
    assert_eq!(answer, Ok(SslChoice::EncryptNoVerify));
    assert!(
        printed.contains("does not check the server certificate"),
        "the warning is printed at the choice: {printed}"
    );
}

#[test]
fn going_back_at_the_ssl_question_stops_the_step() {
    // Four modes, then the way out the prompt appends.
    let (answer, _) = with_ctx("5\n", no_flags(), true, Defaults::empty(), |ctx| {
        ask_ssl(ctx, "db.example.com")
    });
    assert_eq!(answer, Err(Signal::Back));
}

#[test]
fn a_field_takes_its_flag_without_asking() {
    let flags = HashMap::from([("database-host".to_owned(), "db.example.com".to_owned())]);
    let (answer, _) = with_ctx("", flags, true, Defaults::empty(), |ctx| {
        flagged_or_asked(ctx, "database-host", "Host", None, |_| Ok(()))
    });
    assert_eq!(answer, Ok("db.example.com".to_owned()));
}

#[test]
fn a_refused_flag_names_the_flag() {
    let flags = HashMap::from([("database-port".to_owned(), "not-a-port".to_owned())]);
    let (answer, _) = with_ctx("", flags, true, Defaults::empty(), |ctx| {
        flagged_or_asked(ctx, "database-port", "Port", None, |_| {
            Err("is not a number".to_owned())
        })
    });
    let signal = answer.expect_err("the flag is refused");
    assert_eq!(signal.reason(), "--database-port is not a number");
}

#[test]
fn a_non_interactive_field_takes_its_default() {
    let (answer, _) = with_ctx("", no_flags(), false, Defaults::empty(), |ctx| {
        flagged_or_asked(
            ctx,
            "database-port",
            "Port",
            Some("5432".to_owned()),
            |_| Ok(()),
        )
    });
    assert_eq!(answer, Ok("5432".to_owned()));
}

#[test]
fn a_non_interactive_field_with_no_value_says_which_flag_is_missing() {
    let (answer, _) = with_ctx("", no_flags(), false, Defaults::empty(), |ctx| {
        flagged_or_asked(ctx, "database-host", "Host", None, |_| Ok(()))
    });
    let signal = answer.expect_err("there is no value");
    assert!(
        signal
            .reason()
            .contains("--database-host is required without a terminal"),
        "{}",
        signal.reason()
    );
}

#[test]
fn an_interactive_field_takes_the_answered_value() {
    let (answer, _) = with_ctx("5433\n", no_flags(), true, Defaults::empty(), |ctx| {
        flagged_or_asked(
            ctx,
            "database-port",
            "Port",
            Some("5432".to_owned()),
            |_| Ok(()),
        )
    });
    assert_eq!(answer, Ok("5433".to_owned()));
}
