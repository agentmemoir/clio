// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the custody fields a controlled export delivery records:
//! a manifest-backed delivery writes the manifest's copy digest and receiver
//! into the destination custody row, and a delivery without a manifest leaves
//! both fields unset.
//!
//! ## Owns
//! - The manifest fixture and the two custody-field assertions
//!
//! ## Does not own
//! - The delivery module under test
//! - The other delivery refusals and settlement assertions
//!   (`export_custody_tests` owns those)
//!
//! ## Boundary
//!
//! Tests here break when the manifest fields the delivery records change.
//!
//! Test-only. Never compiled into production builds.

use super::export_custody_tests::{NOW, Scratch, accepted, ctx, open_sqlite, spec};
use crate::subject_request_manifest::CopyCustodyManifest;
use crate::{DeliveryResult, deliver_export};
use clio_store::DestinationCustodyStore;

/// A valid manifest digest shape: the algorithm prefix plus 64 hex digits.
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The manifest one test delivery is backed by.
fn manifest() -> CopyCustodyManifest {
    CopyCustodyManifest::write("producer-a", "receiver-b", DIGEST, NOW, NOW)
        .expect("a valid manifest")
}

/// A manifest-backed delivery records the manifest's copy digest and receiver
/// on the custody row.
#[test]
fn a_manifest_backed_delivery_records_the_manifest_digest_and_receiver() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    let staging = scratch.root.join("staging");
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        Some(&manifest()),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(result, DeliveryResult::Delivered);
    let row = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(row.copy_digest.as_deref(), Some(DIGEST), "{row:?}");
    assert_eq!(row.receiver.as_deref(), Some("receiver-b"), "{row:?}");
    assert_eq!(row.state, "delivered");
}

/// A delivery without a manifest leaves the custody digest and receiver
/// unset: only a manifest-backed delivery writes them.
#[test]
fn a_delivery_without_a_manifest_leaves_the_digest_and_receiver_unset() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    let staging = scratch.root.join("staging");
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        None,
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(result, DeliveryResult::Delivered);
    let row = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(row.copy_digest, None, "{row:?}");
    assert_eq!(row.receiver, None, "{row:?}");
    assert_eq!(row.state, "delivered");
}
