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
//! - `serves_untrusted_network`, the trust fact a served bind hands its handler
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
use crate::http::http_start_guard::{
    BindRefusal, HeaderMap, bind_refusal, is_proxied_or_remote, serves_untrusted_network,
};
use crate::http_auth::CredentialStore;
use clio_config::auth_tokens::{AuthTokenRecord, digest_hex};
use clio_types::Actor;
use std::net::SocketAddr;
use std::sync::Arc;

/// A peer off this machine, fabricated rather than reached: 192.0.2.0/24 is the
/// documentation range reserved for exactly this.
fn remote_peer() -> SocketAddr {
    "192.0.2.10:1234".parse().expect("a literal address")
}

/// A peer on this machine.
fn loopback_peer() -> SocketAddr {
    "127.0.0.1:1234".parse().expect("a literal address")
}

/// A header block carrying exactly `headers`, built without a socket.
fn headers(headers: &[(&str, &str)]) -> HeaderMap {
    headers
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

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

/// The transport-trust fact: only a bind outside loopback with no credential
/// source answers peers nobody has vouched for. The address is passed in rather
/// than bound, so the seam under judgement is the predicate.
#[test]
fn only_an_off_machine_credential_free_bind_serves_an_untrusted_network() {
    let no_source = config(true, true);
    assert!(
        serves_untrusted_network(&no_source, false),
        "a non-loopback bind with no credential source is unauthenticated off-machine"
    );
    assert!(
        !serves_untrusted_network(&no_source, true),
        "a loopback bind is reachable only from this machine"
    );
    let sourced = HttpConfig {
        credentials: Some(usable_store()),
        ..config(false, false)
    };
    for loopback in [true, false] {
        assert!(
            !serves_untrusted_network(&sourced, loopback),
            "a credential source vouches for whoever reaches it (loopback {loopback})"
        );
    }
    let empty = HttpConfig {
        credentials: Some(Arc::new(CredentialStore::default())),
        ..config(false, false)
    };
    assert_eq!(
        bind_refusal(&empty, false),
        Some(BindRefusal::NonLoopback),
        "an empty store authorizes nobody, so this bind never serves at all"
    );
}

/// A peer off this machine is remote whatever it sends, and a loopback peer
/// with no forwarding header is local: both branches of the address half.
#[test]
fn an_off_machine_peer_is_proxied_or_remote() {
    assert!(
        is_proxied_or_remote(remote_peer(), &headers(&[])),
        "a peer outside loopback did not arrive locally"
    );
    assert!(
        !is_proxied_or_remote(loopback_peer(), &headers(&[])),
        "a loopback peer that forwards nothing arrived locally"
    );
}

/// Each forwarding header alone is enough, whatever its value, whatever its
/// case, and whichever of the four names it is.
#[test]
fn each_forwarding_header_makes_a_loopback_caller_remote() {
    for name in ["forwarded", "x-forwarded-for", "x-real-ip", "via"] {
        for value in ["203.0.113.7", "", "for=192.0.2.10;proto=https"] {
            let upper = name.to_ascii_uppercase();
            assert!(
                is_proxied_or_remote(loopback_peer(), &headers(&[(name, value)])),
                "`{name}` marks the request forwarded"
            );
            assert!(
                is_proxied_or_remote(loopback_peer(), &headers(&[(&upper, value)])),
                "`{upper}` is the same header in another case"
            );
        }
    }
}

/// A header that forwards nothing leaves a loopback caller local, so the
/// predicate refuses and allows on the header it was given, not on any header.
#[test]
fn an_unrelated_header_leaves_a_loopback_caller_local() {
    let unrelated = headers(&[
        ("host", "localhost"),
        ("user-agent", "clio-test"),
        ("x-request-id", "1"),
    ]);
    assert!(
        !is_proxied_or_remote(loopback_peer(), &unrelated),
        "no forwarding header is present"
    );
}
