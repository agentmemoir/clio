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
