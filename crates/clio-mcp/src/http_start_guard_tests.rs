// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the truth table of the start-time admission
//! decision: for every combination of credential source, bind kind, and
//! opt-out flag, whether a listener is admitted or refused, and why.
//!
//! ## Owns
//! - The loopback and non-loopback rows of `bind_refusal`
//! - The rule that a non-loopback bind with no credential needs both opt-outs
//!
//! ## Does not own
//! - The refusal wording and the transport itself (see `http_start_auth_tests`
//!   and `http_tests`)
//! - The credential-edge rules (see `http_auth_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::http::HttpConfig;
use crate::http::http_start_guard::{BindRefusal, bind_refusal};
use crate::http_auth::CredentialStore;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;
use std::sync::Arc;

/// A config with no credential store and the two opt-outs as given.
fn config(insecure_no_auth: bool, insecure_bind_any: bool) -> HttpConfig {
    HttpConfig {
        insecure_no_auth,
        insecure_bind_any,
        ..HttpConfig::default()
    }
}

/// A store holding one record: the only shape that counts as a source.
fn usable_store() -> Arc<CredentialStore> {
    Arc::new(CredentialStore::from_records(vec![AuthTokenRecord {
        label: "ci".into(),
        secret_sha256: digest_hex("secret-token"),
        actor: Actor::Harness,
        banks: vec!["demo".into()],
        created_at: "2026-01-01T00:00:00Z".into(),
    }]))
}

#[test]
fn loopback_without_a_credential_is_refused() {
    assert_eq!(
        bind_refusal(&config(false, false), true),
        Some(BindRefusal::Loopback)
    );
}

/// The one opt-out is the loopback answer, and the second flag neither adds to
/// nor subtracts from it.
#[test]
fn loopback_with_the_opt_out_is_admitted() {
    for insecure_bind_any in [false, true] {
        assert_eq!(
            bind_refusal(&config(true, insecure_bind_any), true),
            None,
            "loopback is admitted under insecure_no_auth (bind_any {insecure_bind_any})"
        );
    }
}

#[test]
fn non_loopback_without_a_credential_is_refused() {
    assert_eq!(
        bind_refusal(&config(false, false), false),
        Some(BindRefusal::NonLoopback)
    );
}

/// The bug: `--insecure-no-auth` alone refused nothing, so a non-loopback bind
/// served every interface with no credential. It is the loopback answer only.
#[test]
fn non_loopback_with_insecure_no_auth_alone_is_refused() {
    assert_eq!(
        bind_refusal(&config(true, false), false),
        Some(BindRefusal::NonLoopback)
    );
}

/// The second flag on its own is not an opt-out either: the pair is required.
#[test]
fn non_loopback_with_insecure_bind_any_alone_is_refused() {
    assert_eq!(
        bind_refusal(&config(false, true), false),
        Some(BindRefusal::NonLoopback)
    );
}

#[test]
fn non_loopback_with_both_opt_outs_is_admitted() {
    assert_eq!(bind_refusal(&config(true, true), false), None);
}

/// A store holding a record authorizes requests, so the bind is admitted
/// whatever the opt-outs say.
#[test]
fn a_usable_store_admits_every_bind() {
    for insecure_no_auth in [false, true] {
        for insecure_bind_any in [false, true] {
            let cfg = HttpConfig {
                credentials: Some(usable_store()),
                ..config(insecure_no_auth, insecure_bind_any)
            };
            for loopback in [true, false] {
                assert_eq!(
                    bind_refusal(&cfg, loopback),
                    None,
                    "a non-empty store is a source (loopback {loopback}, \
                     no_auth {insecure_no_auth}, bind_any {insecure_bind_any})"
                );
            }
        }
    }
}

/// An empty store authorizes nothing, so it is not a source: it must not admit
/// a bind the absent store would refuse.
#[test]
fn an_empty_store_admits_nothing() {
    let cfg = HttpConfig {
        credentials: Some(Arc::new(CredentialStore::default())),
        ..config(false, false)
    };
    assert_eq!(bind_refusal(&cfg, true), Some(BindRefusal::Loopback));
    assert_eq!(bind_refusal(&cfg, false), Some(BindRefusal::NonLoopback));
}
