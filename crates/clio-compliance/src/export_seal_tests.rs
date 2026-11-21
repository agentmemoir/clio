// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the recipient-seal tests: the X25519 round
//! trip, malformed recipient text, a tampered ciphertext, and the
//! readable-export opt-in gate that records a durable event before delivery.
//!
//! ## Owns
//! - Seal, opt-in refusal, and sealed-payload assertions over the shared export
//!   fixtures
//!
//! ## Does not own
//! - Fixture builders (see `export_tests.rs`)
//! - The export writer itself (see `export.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use base64::Engine as _;

use crate::bundle::ExportContentMode;
use crate::export::{ExportArgs, ExportDelivery, ExportFilter, export};
use crate::export_seal::seal_to_recipient;
use crate::export_tests::{Scratch, args, ctx, fixture, open_sealed, read_sealed, test_recipient};
use crate::subject_request_events::BindingControls;
use clio_store::LocalEventStore;
use clio_types::{Actor, ErrorCode};

/// Text that must never appear in a sealed file. It stands for readable memory
/// content on its way to disk.
const SENTINEL: &[u8] = b"sentinel-sealed-export";

#[test]
fn seal_round_trips_to_the_recipient_identity() {
    let sealed = seal_to_recipient(SENTINEL, &test_recipient()).expect("seal");
    assert!(
        !sealed
            .windows(SENTINEL.len())
            .any(|window| window == SENTINEL)
    );
    assert!(
        !sealed.windows(4).any(|window| window == b"gist"),
        "the plaintext gist is not on disk",
    );
    let opened = open_sealed(&sealed).expect("open");
    assert_eq!(opened, SENTINEL);
}

#[test]
fn seal_rejects_malformed_recipient_text() {
    let error = seal_to_recipient(SENTINEL, "not-a-key!!!").expect_err("refuse");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn seal_rejects_recipient_text_of_the_wrong_length() {
    let short = base64::engine::general_purpose::STANDARD.encode([7u8; 31]);
    let error = seal_to_recipient(SENTINEL, &short).expect_err("refuse");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[test]
fn tampered_ciphertext_does_not_open() {
    let mut sealed = seal_to_recipient(SENTINEL, &test_recipient()).expect("seal");
    let last = sealed.len() - 1;
    sealed[last] ^= 0xff;
    assert!(
        open_sealed(&sealed).is_err(),
        "a tampered seal must not open"
    );
}

#[test]
fn readable_export_without_the_opt_in_is_refused() {
    let f = fixture("seal-no-optin");
    let ctx = ctx(&f, Actor::Harness);
    let error = export(&ExportArgs {
        readable_export_opt_in: false,
        ..args(
            &f,
            &ctx,
            ExportFilter::default(),
            ExportContentMode::DsarPlaintext,
        )
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert!(!f.path.as_path().exists());
}

#[test]
fn readable_export_without_a_recipient_is_refused() {
    let f = fixture("seal-no-recipient");
    let ctx = ctx(&f, Actor::Harness);
    let error = export(&ExportArgs {
        recipient: None,
        ..args(
            &f,
            &ctx,
            ExportFilter::default(),
            ExportContentMode::DsarPlaintext,
        )
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert!(!f.path.as_path().exists());
}

#[test]
fn malformed_recipient_is_rejected_before_any_write() {
    let f = fixture("seal-bad-recipient");
    let ctx = ctx(&f, Actor::Harness);
    let error = export(&ExportArgs {
        recipient: Some("not-a-key!!!".to_owned()),
        ..args(
            &f,
            &ctx,
            ExportFilter::default(),
            ExportContentMode::DsarPlaintext,
        )
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(!f.path.as_path().exists());
}

#[test]
fn short_recipient_is_rejected_before_any_write() {
    let f = fixture("seal-short-recipient");
    let ctx = ctx(&f, Actor::Harness);
    let error = export(&ExportArgs {
        recipient: Some(base64::engine::general_purpose::STANDARD.encode([7u8; 31])),
        ..args(
            &f,
            &ctx,
            ExportFilter::default(),
            ExportContentMode::DsarPlaintext,
        )
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(!f.path.as_path().exists());
}

#[test]
fn readable_export_is_sealed_to_the_recipient_key() {
    let f = fixture("seal-content");
    let ctx = ctx(&f, Actor::Harness);
    export(&args(
        &f,
        &ctx,
        ExportFilter::default(),
        ExportContentMode::DsarPlaintext,
    ))
    .expect("sealed export delivers");
    let raw = std::fs::read(f.path.as_path()).expect("read the sealed file");
    assert!(!raw.windows(4).any(|window| window == b"gist"));
    let opened = read_sealed(f.path.as_path());
    assert!(opened.contains("plain gist"));
}

#[test]
fn readable_export_records_the_opt_in_event() {
    let f = fixture("seal-events");
    let ctx = ctx(&f, Actor::Harness);
    let scratch = Scratch::new();
    let dest = scratch.destination();
    let dest_str = dest.to_string_lossy().into_owned();
    let controls = BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    };
    let staging = scratch.staging_dir();
    export(&ExportArgs {
        destination: &dest_str,
        delivery: Some(ExportDelivery {
            events: &f.store,
            custody: &f.store,
            controls: &controls,
            action_id: "act-seal",
            staging_dir: &staging,
            custody_owner: "clio",
            required_action: "delivery",
        }),
        ..args(
            &f,
            &ctx,
            ExportFilter::default(),
            ExportContentMode::DsarPlaintext,
        )
    })
    .expect("sealed export delivers");
    let events = f.store.list_events().expect("list events");
    assert!(
        events.iter().any(|event| {
            event.kind == "export_delivery"
                && event.action_id == "act-seal"
                && event.scope == "export_readable_optin"
        }),
        "the delivery intent records the readable opt-in: {events:?}",
    );
}
