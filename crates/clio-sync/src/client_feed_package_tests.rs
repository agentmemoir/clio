// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the packaging helpers of the push
//! change feed that live in `client_feed_package`, at the unit layer: the
//! cipher choice for one mutation's content and the identity of the key that
//! sealed it.
//!
//! ## Owns
//! - The tests for `content_for`: the empty-list (encryption off) case and
//!   the sealed case naming the key that sealed
//!
//! ## Does not own
//! - Seal/open correctness and nonce behaviour (`crypto_tests`)
//! - The end-to-end push path with a real server (`client_tests`,
//!   `client_rotation_tests`)
//! - Feed ordering and cursors (`client_feed_wm_tests`)
//!
//! ## Boundary
//!
//! Changes to what `content_for` promises should normally require changes
//! here. Changes to the AEAD itself should not.
//!
//! Keep this module focused on content-for packaging tests.

use serde_json::json;

use crate::client::client_tests::{cfg, sqlite};
use crate::client_feed::package::content_for;
use crate::crypto::SyncCipherKey;
use crate::protocol::SyncContent;

/// An empty key list means encryption off: the content leaves as `Plain`,
/// byte-identical, and no cipher is sealed.
#[test]
fn an_empty_list_leaves_content_plain() {
    let store = sqlite();
    let cfg = cfg("feed-plain", &store, "127.0.0.1:1", Vec::new());
    let content = json!({ "gist": "plain content", "n": 1 });
    let got = content_for(&cfg, &content).expect("package");
    match got {
        SyncContent::Plain(value) => assert_eq!(value, content, "plain content is carried as is"),
        SyncContent::Cipher(_) => panic!("an empty key list must not seal"),
    }
}

/// The first entry of the list is the sealing key: the sealed content names
/// that key's identifier and opens under it, byte-for-byte.
#[test]
fn content_seals_with_the_first_entry_of_the_list() {
    let store = sqlite();
    let newest = SyncCipherKey::from_secret("feed-key-newest");
    let older = SyncCipherKey::from_secret("feed-key-older");
    let cfg = cfg(
        "feed-sealed",
        &store,
        "127.0.0.1:1",
        vec![newest.clone(), older],
    );
    let content = json!({ "gist": "sealed content", "n": 2 });
    let got = content_for(&cfg, &content).expect("package");
    let cipher = match got {
        SyncContent::Cipher(cipher) => cipher,
        SyncContent::Plain(_) => panic!("a held key must seal"),
    };
    assert_eq!(
        cipher.key_id.as_deref(),
        Some(newest.key_id().as_str()),
        "the newest key, the first entry, is the sealing key"
    );
    let opened = newest.open(&cipher).expect("open under the sealing key");
    assert_eq!(
        opened,
        serde_json::to_vec(&content).expect("serialize"),
        "the sealed bytes are the serialized content"
    );
}
