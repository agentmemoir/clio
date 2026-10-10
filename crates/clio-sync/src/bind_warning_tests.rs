// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the bind warning: the loopback
//! classification across every address class an operator binds, and which side
//! of the cleartext/sealed line each documented field is named on.
//!
//! ## Owns
//! - Behavior coverage for `bind_warning`
//!
//! ## Does not own
//! - The emission wiring at the bind site (`server` tests, and the real-binary
//!   harness in `clio`)
//! - The documented set's consistency with the wire (`client_feed_documentation_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the classification or the warning text changes; they
//! never change it.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::field_documentation_test_support::{
    KEYED_CLAUSE_START, ROUTING_CLAUSE_START, SEALED_CLAUSE_START,
    assert_each_field_is_named_on_its_side,
};

fn addr(text: &str) -> SocketAddr {
    text.parse().expect("parse test address")
}

/// Every loopback class stays silent: `127.0.0.1` and the rest of its range,
/// and `::1`.
#[test]
fn loopback_addresses_classify_as_loopback() {
    for text in ["127.0.0.1:1", "127.255.255.254:1", "[::1]:1"] {
        assert!(bind_is_loopback(addr(text)), "{text} is loopback");
    }
}

/// The unspecified addresses are the case an operator gets wrong: they bind
/// every interface, so they are not loopback and they warn. A routable
/// address warns too.
#[test]
fn unspecified_and_routable_addresses_classify_as_public() {
    for text in ["0.0.0.0:1", "[::]:1", "192.168.1.5:1", "192.0.2.1:1"] {
        assert!(!bind_is_loopback(addr(text)), "{text} is not loopback");
    }
}

/// The warning names every documented field inside the clause that field's side
/// names, so a field that crosses the network in the clear cannot be presented
/// as sealed by being mentioned somewhere in the sentence. The shared assertion
/// locates a name by the clause it appears in rather than by the whole string,
/// which is the property that was missing when the record identifier and the
/// content digest were listed as sealed while reading in the clear.
#[test]
fn the_warning_names_each_field_on_the_side_it_sits_on() {
    let warning = public_bind_warning(addr("0.0.0.0:34311"));
    assert_each_field_is_named_on_its_side(&warning);
}

/// The warning carries the reason for each of its clauses, so an operator can
/// act on it rather than dismiss it.
#[test]
fn the_warning_carries_the_reason_for_each_clause() {
    let warning = public_bind_warning(addr("0.0.0.0:34311"));
    assert!(
        warning.contains(ROUTING_CLAUSE_START),
        "the routing clause must open: {warning}"
    );
    assert!(
        warning.contains("routing-necessary, not harmless"),
        "the residual must be named as such rather than presented as harmless: {warning}"
    );
    assert!(
        warning.contains(KEYED_CLAUSE_START),
        "the keyed-cleartext clause must open: {warning}"
    );
    assert!(
        warning.contains("derived under a key the relay does not hold"),
        "the keyed clause names the mechanism, not a claim of unguessability: {warning}"
    );
    assert!(
        warning.contains("confirm a guessed value against them"),
        "the keyed clause states what guessing does not get a relay: {warning}"
    );
    assert!(
        warning.contains("one stable value per row"),
        "the keyed clause states what a relay does keep: {warning}"
    );
    assert!(
        warning.contains("an identifier the machine generated is sent unchanged"),
        "the keyed clause names the kind that carries no handle: {warning}"
    );
    assert!(
        warning.contains(SEALED_CLAUSE_START),
        "the sealed clause must open: {warning}"
    );
}

/// The warning is printed before any request exists, so it must not carry a
/// secret, and its sealed clause must not name a field that reads in the clear.
#[test]
fn the_warning_carries_no_secret_and_no_cleartext_field_in_the_sealed_clause() {
    let warning = public_bind_warning(addr("0.0.0.0:34311"));
    assert!(
        !warning.contains("token") && !warning.contains("Bearer"),
        "the warning never names auth material: {warning}"
    );
    let sealed = &warning[warning.find(SEALED_CLAUSE_START).expect("a sealed clause")..];
    assert!(
        !sealed.contains("record identifier") && !sealed.contains("content digest"),
        "the sealed clause names only the content and the score: {sealed}"
    );
}
