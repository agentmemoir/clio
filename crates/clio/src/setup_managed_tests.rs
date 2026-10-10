// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the values a managed Postgres is given
//! and the port it is offered: that they are fixed, that the range is the one
//! documented, and that a free port is actually reported free.
//!
//! ## Owns
//! - Every test of `setup_managed`
//!
//! ## Does not own
//! - The wizard question that shows these values (see `setup_flows_db`)
//! - The container configuration that consumes them (see `compose_env`)
//!
//! # Why one test occupies the whole port range
//!
//! The function under test binds real ports, so a test that provokes its refusal
//! has to hold every port it would otherwise take. That is the only way to reach
//! the refusal at all, and the listeners are released when the test ends.
//!
//! Holding the whole range means no other test that probes the same range can run
//! at the same time, in either direction: this test needs every port free of a
//! sibling, and the sibling below needs one port free of this test. The runner
//! gives every test its own process, so an in-process lock cannot order them. The
//! runner's configuration therefore collects every test that binds or probes this
//! range into one group that runs a single thread at a time.

use super::*;
use std::net::TcpListener;

#[test]
fn the_managed_values_match_the_container_configuration() {
    // These three are written into `compose.yml` as `CLIO_POSTGRES_USER` and
    // `CLIO_POSTGRES_DB`, and the host is loopback because the container publishes
    // on this machine only.
    assert_eq!(MANAGED_USER, "clio", "the role the container creates");
    assert_eq!(
        MANAGED_DATABASE, "clio",
        "the database the container creates"
    );
    assert_eq!(
        MANAGED_HOST, "127.0.0.1",
        "the container is published on loopback, not on a resolvable name"
    );
}

#[test]
fn the_user_and_database_are_the_ones_the_generated_env_writes() {
    // The comment above the constants claims the two sides match. This is where
    // that claim is proved: the generated `.env` and the wizard's constants come
    // from two modules, so a change to either can silently disagree with the
    // other and leave the container creating a role the store will not dial.
    // The argument is the embed model, which neither line below depends on.
    let contents = crate::compose_env::env_contents("test-model");
    for line in [
        format!("CLIO_POSTGRES_USER={MANAGED_USER}"),
        format!("CLIO_POSTGRES_DB={MANAGED_DATABASE}"),
    ] {
        assert!(
            contents.contains(&line),
            "the generated env must write `{line}`"
        );
    }
}

#[test]
fn the_database_is_never_named_after_the_machine_user() {
    // A database named after whoever ran setup is how two installs end up with
    // different names for the same role, and how a restore lands in the wrong
    // place. The name is a constant for that reason.
    assert_ne!(
        MANAGED_DATABASE, "root",
        "the database name is not the operator's name"
    );
    assert_eq!(MANAGED_DATABASE, MANAGED_USER);
}

#[test]
fn the_preferred_port_is_inside_the_range_and_is_its_highest() {
    assert!(
        (LOWEST_PORT..=PREFERRED_PORT).contains(&PREFERRED_PORT),
        "the preferred port {PREFERRED_PORT} is outside {LOWEST_PORT}..={PREFERRED_PORT}"
    );
    assert_eq!(
        PREFERRED_PORT, 34310,
        "the preferred port is what compose.yml already defaults to"
    );
}

#[test]
fn the_port_range_is_eleven_ports_wide_starting_at_34300() {
    assert_eq!(LOWEST_PORT, 34300);
    let ports = ports_in_preference_order();
    assert_eq!(ports.len(), 11, "the range is 34300 to 34310 inclusive");
    assert_eq!(
        ports.first(),
        Some(&PREFERRED_PORT),
        "preferred is tried first"
    );
    assert_eq!(ports.last(), Some(&LOWEST_PORT), "lowest is tried last");
}

#[test]
fn every_port_in_the_order_is_inside_the_range_and_appears_once() {
    let ports = ports_in_preference_order();
    for port in &ports {
        assert!(
            (LOWEST_PORT..=PREFERRED_PORT).contains(port),
            "{port} is outside the documented range"
        );
    }
    let mut sorted = ports.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        ports.len(),
        "a repeated port would be tried twice for no reason"
    );
}

#[test]
fn a_port_the_function_reports_can_be_bound() {
    // The claim under test is that the reported port is free, so it is proved by
    // binding it. The listener is dropped before the assertion's own bind, which
    // is the same thing the production code does.
    let reported = pick_managed_port().expect("at least one port in the range is free");
    let port: u16 = reported.parse().expect("the port is a number");
    assert!(
        (LOWEST_PORT..=PREFERRED_PORT).contains(&port),
        "{port} is outside the range the function documents"
    );
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("the reported port is free");
}

#[test]
fn the_exhaustion_message_names_the_whole_range() {
    // Every port the function may choose is held for the length of this test, so
    // it has nowhere free to go and has to refuse. A port this test cannot bind is
    // a port another process already holds, which leaves the range just as
    // occupied, so a failed bind changes nothing about what the function must do.
    let _held: Vec<TcpListener> = ports_in_preference_order()
        .iter()
        .filter_map(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, *port)).ok())
        .collect();

    // The claim under test is the wording of the refusal, so what is asserted on
    // is the error this call returns. A copy of the text built here would pass
    // even if the function refused with different words, which is how a test can
    // look like coverage of this arm while never entering it.
    let message = pick_managed_port().expect_err("every port in the range is held");
    assert!(
        message.contains("34300"),
        "the refusal names the lowest port of the range, so the operator knows \
         which to free: {message}"
    );
    assert!(
        message.contains("34310"),
        "the refusal names the highest port of the range too: {message}"
    );
    assert!(
        message.contains("Postgres you already have"),
        "the refusal offers the way out of the refusal: {message}"
    );
}
