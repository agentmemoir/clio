// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests one denial operation's delivery to one peer over real
//! TCP: a peer that records durably and answers with a verified proof is
//! delivered, a proof under another deployment salt is refused, a transport
//! answer without a proof is never accepted, a peer's refusal (4xx) and a
//! retryable answer (5xx) stay distinct, an unreachable peer reports
//! unreachable, malformed input is refused before any exchange, and a
//! redelivery returns the same proof without a second effect.
//!
//! ## Owns
//! - Behavior coverage for `deliver_denial` and its outcome classification
//!
//! ## Does not own
//! - The server's denial routes (`server_denial_tests`) or the wire vocabulary
//!   (`denial_tests`)
//!
//! ## Boundary
//!
//! These tests break when one delivery's classification changes; they never
//! change it.
//!
//! Keep this module focused on the delivery surface.

use crate::client::SyncClientConfig;
use crate::credential_fixtures::{Scratch, record, secret};
use crate::delivery::{DeliveryOutcome, DenialTarget, deliver_denial};
use crate::denial::denied_bank_marker;
use crate::denial_fixtures::FakeDenial;
use crate::server::server_tests::{serve, sqlite};
use crate::server::{SyncServer, SyncServerOptions, sync_serve_with_denial};
use clio_store::{ErasureDenial, SqliteStore, Store, SyncStore};
use clio_types::ErrorCode;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

/// A running server with the authority wired, its store, the authority, and
/// the credential the tests present.
fn serve_denied() -> (
    SyncServer,
    Arc<SqliteStore>,
    Arc<FakeDenial>,
    Scratch,
    String,
) {
    let scratch = Scratch::new("deliver-denial");
    let token = secret("d");
    scratch.write(&[record("client-a", &token, &["b-a", "b-b"])]);
    let (store, kms) = sqlite();
    let denial = FakeDenial::new("fp-1");
    let server = sync_serve_with_denial(
        SyncServerOptions {
            bind: "127.0.0.1:0".into(),
            store: Arc::clone(&store) as Arc<dyn Store>,
            sync: Arc::clone(&store) as Arc<dyn SyncStore>,
            kms,
            banks: vec!["b-a".into(), "b-b".into()],
            token_file: Some(scratch.path().to_path_buf()),
            dev_mode: false,
            ttl_secs: None,
            deadline_secs: None,
            device_id: "srv-1".into(),
        },
        Arc::clone(&denial) as Arc<dyn ErasureDenial>,
    )
    .expect("serve");
    (server, store, denial, scratch, token)
}

/// A client config pointing at `remote` with `token`, over its own store.
fn client(remote: &str, token: &str) -> SyncClientConfig {
    let (store, _kms) = sqlite();
    SyncClientConfig {
        device_id: "dev-a".into(),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: store as Arc<dyn SyncStore>,
        remote: Some(remote.to_owned()),
        token: Some(token.to_owned()),
        cipher: Vec::new(),
    }
}

/// The one delivery target, verified against `fingerprint`.
fn target(fingerprint: &str) -> DenialTarget<'_> {
    DenialTarget {
        request_ref: "req-1",
        peer_id: "srv-1",
        bank: "b-a",
        subject_id: "s-held",
        covering: &["b-a", "b-b"],
        salt_fingerprint: fingerprint,
    }
}

/// A peer that answers one push with a fixed status and body, then closes.
fn answering_peer(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _peer)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let answer = format!(
                "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    (addr, handle)
}

/// A delivery without a peer endpoint is the caller's own input: no transport
/// answer exists to classify.
#[test]
fn a_delivery_without_a_remote_endpoint_is_input_refused() {
    let (store, _kms) = sqlite();
    let cfg = SyncClientConfig {
        device_id: "dev-a".into(),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: store as Arc<dyn SyncStore>,
        remote: None,
        token: None,
        cipher: Vec::new(),
    };
    let err = deliver_denial(&cfg, &target("fp-1")).expect_err("no remote is the caller's input");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// Every required target field is refused when blank, before any exchange.
#[test]
fn a_blank_target_field_is_input_refused() {
    let cfg = client("127.0.0.1:1", "tok");
    for delivery in [
        DenialTarget {
            request_ref: "",
            ..target("fp-1")
        },
        DenialTarget {
            peer_id: "",
            ..target("fp-1")
        },
        DenialTarget {
            bank: "",
            ..target("fp-1")
        },
        DenialTarget {
            subject_id: "",
            ..target("fp-1")
        },
        DenialTarget {
            salt_fingerprint: "  ",
            ..target("fp-1")
        },
    ] {
        let err = deliver_denial(&cfg, &delivery).expect_err("a blank field is the caller's input");
        assert_eq!(err.code, ErrorCode::InvalidArgument);
    }
}

/// A target covering no bank cannot build a carrier: the refusal is input, not
/// a delivery outcome.
#[test]
fn a_target_covering_no_bank_is_input_refused() {
    let cfg = client("127.0.0.1:1", "tok");
    let empty = DenialTarget {
        covering: &[],
        ..target("fp-1")
    };
    let err = deliver_denial(&cfg, &empty).expect_err("a carrier needs one covered bank");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// A peer that answers a 4xx refusal keeps the action pending as a refusal,
/// never as a delivery.
#[test]
fn a_peer_refusal_is_refused_not_pending() {
    let (server, _store) = serve(&["b-a"], Some("tok-plain"), false, None, None);
    let cfg = client(server.addr(), "tok-wrong");
    let outcome = deliver_denial(&cfg, &target("fp-1")).expect("deliver");
    assert!(
        matches!(&outcome, DeliveryOutcome::Refused { .. }),
        "a 4xx answer is a refusal, never a delivery: {outcome:?}"
    );
    server.stop();
}

/// A retryable non-4xx answer has no durable effect at the peer: the action
/// stays pending.
#[test]
fn a_retryable_peer_answer_is_unreachable() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let cfg = client(server.addr(), &token);
    denial.damage();
    let outcome = deliver_denial(&cfg, &target("fp-1")).expect("deliver");
    assert!(
        matches!(&outcome, DeliveryOutcome::Unreachable { .. }),
        "a 5xx keeps the action pending: {outcome:?}"
    );
    assert_eq!(
        denial.records_for("s-held"),
        0,
        "a damaged authority records nothing"
    );
    server.stop();
}

/// A 200 whose body is not an acknowledgement is never delivered: without a
/// durable proof there is nothing to retire the action on.
#[test]
fn an_unreadable_acknowledgement_is_not_delivered() {
    let (addr, peer) = answering_peer(200, "{\"not\":\"an acknowledgment\"}");
    let cfg = client(&addr, "tok");
    let outcome = deliver_denial(&cfg, &target("fp-1")).expect("deliver");
    assert!(
        matches!(&outcome, DeliveryOutcome::Unreachable { .. }),
        "an unreadable acknowledgment is not a delivery: {outcome:?}"
    );
    peer.join().expect("the answer thread ends");
}

/// A durable proof that verifies is a delivery, and the peer's durable record
/// exists: the caller may retire the action.
#[test]
fn an_accepted_proof_is_delivered_with_the_peers_durable_record() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let cfg = client(server.addr(), &token);
    let outcome = deliver_denial(&cfg, &target("fp-1")).expect("deliver");
    let DeliveryOutcome::Delivered { proof } = outcome else {
        panic!("expected a delivered proof: {outcome:?}");
    };
    assert_eq!(proof.request_ref, "req-1");
    assert_eq!(proof.peer_id, "srv-1");
    assert_eq!(proof.salt_fingerprint, "fp-1");
    assert!(
        proof.generation > 0,
        "a proof carries a recorded generation"
    );
    assert_eq!(denial.records_for("s-held"), 1, "one durable record");
    assert_eq!(
        denial.records_for(&denied_bank_marker("b-a")),
        1,
        "the served bank's guard is durable too"
    );
    server.stop();
}

/// A proof answered under another deployment salt is refused, even though the
/// peer answered 200 and recorded its own denial.
#[test]
fn a_mismatched_fingerprint_is_refused() {
    let (server, _store, denial, _scratch, token) = serve_denied();
    let cfg = client(server.addr(), &token);
    let outcome = deliver_denial(&cfg, &target("fp-other")).expect("deliver");
    assert!(
        matches!(&outcome, DeliveryOutcome::Refused { .. }),
        "a proof under another salt is never delivered: {outcome:?}"
    );
    assert_eq!(
        denial.records_for("s-held"),
        1,
        "the peer recorded durably; the caller refused the answer"
    );
    server.stop();
}

/// A transport 200 without a durable proof is refused: a peer with no
/// authority wired dead-letters the operation and acknowledges nothing.
#[test]
fn a_transport_answer_without_a_proof_is_not_accepted() {
    let (server, _store) = serve(&["b-a"], Some("tok-plain"), false, None, None);
    let cfg = client(server.addr(), "tok-plain");
    let outcome = deliver_denial(&cfg, &target("fp-1")).expect("deliver");
    assert!(
        matches!(&outcome, DeliveryOutcome::Refused { .. }),
        "a 200 without a durable proof is never delivered: {outcome:?}"
    );
    server.stop();
}

/// A peer that answers no request at all has no durable effect: the caller
/// keeps the action pending.
#[test]
fn a_peer_that_answers_nothing_reports_unreachable() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let closer = std::thread::spawn(move || {
        if let Ok((stream, _peer)) = listener.accept() {
            // Close without reading: the delivery sees no answer.
            drop(stream);
        }
    });
    let cfg = client(&addr, "tok-plain");
    let outcome = deliver_denial(&cfg, &target("fp-1")).expect("deliver");
    assert!(
        matches!(&outcome, DeliveryOutcome::Unreachable { .. }),
        "no answer is not a delivery: {outcome:?}"
    );
    closer.join().expect("the closer thread ends");
}

/// A redelivery of the same action returns the same proof and repeats no
/// effect: one durable record and one journal row.
#[test]
fn a_second_delivery_returns_the_same_proof_without_a_second_effect() {
    let (server, store, denial, _scratch, token) = serve_denied();
    let cfg = client(server.addr(), &token);
    let first = deliver_denial(&cfg, &target("fp-1")).expect("first deliver");
    let second = deliver_denial(&cfg, &target("fp-1")).expect("second deliver");
    assert!(
        matches!(&first, DeliveryOutcome::Delivered { .. }),
        "{first:?}"
    );
    assert_eq!(first, second, "a redelivery returns the same proof");
    assert_eq!(denial.records_for("s-held"), 1, "no second effect");
    let rows = store
        .journal_list_since("b-a", 0, 100)
        .expect("journal rows");
    assert_eq!(rows.len(), 1, "one relayed journal row");
    server.stop();
}
