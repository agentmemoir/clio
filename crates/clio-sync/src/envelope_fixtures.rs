// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the one way a test fixture seals content for a mutation it
//! is about to send: under that mutation's own execution envelope. Shared by
//! the test modules that hand-push a sealed row, and holding no test of its own.
//!
//! ## Owns
//! - `seal_content_for`, the fixture-side seal bound to a mutation's envelope
//! - `seal_own_content` and `opened_document`, the shared-key shorthand the
//!   hand-pushing fixtures and the pull assertions use
//!
//! ## Does not own
//! - The envelope encoding itself (`envelope`)
//! - Packaging a real mutation (`client_feed_package`)
//!
//! ## Boundary
//!
//! Changes to how a fixture seals content should normally require changes here.
//! Changes to what a test asserts about the row it pushed should not.
//!
//! Keep this module focused on fixture sealing.

use crate::client::client_tests::paired_key;
use crate::crypto::{SyncCipher, SyncCipherKey};
use crate::protocol::{SyncContent, SyncMutation};
use serde_json::Value;

/// Seal `document` for `mutation`, exactly as the packagers do: the associated
/// data is the mutation's own execution envelope, and the content is set last
/// because it is excluded from that envelope.
///
/// # Panics
/// Serializing a `Value` and sealing a short document cannot fail.
pub(crate) fn seal_content_for(
    mut mutation: SyncMutation,
    key: &SyncCipherKey,
    document: &Value,
) -> SyncMutation {
    let bytes = serde_json::to_vec(document).expect("serialize the mutation document");
    let aad = mutation.execution_view().aad();
    let cipher: SyncCipher = key.seal(&bytes, &aad).expect("seal");
    mutation.content = SyncContent::Cipher(cipher);
    mutation
}

/// `mutation`'s own plaintext content sealed under the one shared key: what a
/// hand-pushing fixture does to a row the route would otherwise refuse, so the
/// row travels exactly as a client-feeding node sends it.
///
/// A mutation that already carries sealed content is returned unchanged.
pub(crate) fn seal_own_content(mutation: SyncMutation) -> SyncMutation {
    match &mutation.content {
        SyncContent::Plain(document) => {
            let document = document.clone();
            seal_content_for(mutation, &paired_key(), &document)
        }
        SyncContent::Cipher(_) => mutation,
    }
}

/// The document a pulled sealed row carries, opened with the one shared key.
///
/// # Panics
/// A row that is not a sealed envelope this key opens, or a seal that does not
/// authenticate under the row's own envelope.
pub(crate) fn opened_document(row: &Value) -> Value {
    let mutation: SyncMutation = serde_json::from_value(row.clone()).expect("pulled mutation");
    let SyncContent::Cipher(cipher) = &mutation.content else {
        panic!("a sealed row must come back sealed: {row}");
    };
    let bytes = paired_key()
        .open(cipher, &mutation.execution_view().aad())
        .expect("the shared key opens the served seal");
    serde_json::from_slice(&bytes).expect("the opened document")
}
