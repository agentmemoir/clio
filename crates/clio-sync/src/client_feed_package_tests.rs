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
//! - The tests for `content_for`: the keyless refusal, the explicit
//!   local-development plaintext mode, and the sealed case naming the key that
//!   sealed
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

/// A fixed associated data for this packaging-level test: it exercises the
/// key choice, not the envelope binding.
const TEST_AAD: &[u8] = b"am-sync-test-aad";

use crate::client::client_tests::{cfg, sqlite};
use crate::client_feed::package::content_for;
use crate::crypto::SyncCipherKey;
use crate::protocol::SyncContent;
use clio_types::ErrorCode;

/// The packaging functions put one encoding of the writer stamp on the wire:
/// canonical ISO-8601 UTC at whole seconds, whatever legend the store column
/// carries. The store stamps an association edge with epoch millis, so a row
/// taken straight from the feed is the case that has to be normalised.
#[test]
fn a_millisecond_store_stamp_carries_iso_seconds_on_the_wire() {
    use crate::client_feed::package::assoc_mutation;
    use crate::client_feed_package_fixtures::{BANK, MILLIS_STAMP, MILLIS_STAMP_ISO, millis_edge};
    let cfg = crate::client_feed_package_fixtures::plain_cfg();
    let edge = millis_edge("e-millis");
    assert_eq!(
        edge.last_updated_at, MILLIS_STAMP,
        "the fixture column is millis"
    );
    let m = assoc_mutation(&cfg, BANK, &edge).expect("package the millis edge");
    assert_eq!(
        m.transaction_time, MILLIS_STAMP_ISO,
        "the wire carries the canonical second, not the store's millis text"
    );
}

/// A node that holds no sync key refuses the export instead of putting memory on
/// the wire unsealed. The refusal is a configuration error that names the
/// missing key, the remedy, and the flag that exists only for development.
#[test]
fn a_node_with_no_key_refuses_the_export() {
    let store = sqlite();
    let cfg = cfg("feed-keyless", &store, "127.0.0.1:1", Vec::new());
    let content = json!({ "gist": "content that must not leave unsealed", "n": 1 });
    let err = content_for(&cfg, &content, TEST_AAD).expect_err("a keyless export is refused");
    assert_eq!(
        err.code,
        ErrorCode::ConfigCorrupt,
        "the refusal is reported as a configuration error: {}",
        err.message
    );
    assert!(
        err.message.contains("no sync key"),
        "the message names the missing key: {}",
        err.message
    );
    assert!(
        err.message
            .contains("never exported without client-side encryption"),
        "the message states memory is never sent unencrypted: {}",
        err.message
    );
    assert!(
        err.message.contains("clio sync keygen"),
        "the message names the remedy: {}",
        err.message
    );
    assert!(
        err.message.contains("allow_plaintext"),
        "the message names the development-only flag: {}",
        err.message
    );
}

/// The explicit local-development flag keeps the plaintext mode: the content
/// leaves as `Plain`, byte-identical, and no cipher is sealed.
#[test]
fn the_development_flag_leaves_content_plain() {
    let store = sqlite();
    let mut cfg = cfg("feed-plain", &store, "127.0.0.1:1", Vec::new());
    cfg.allow_plaintext = true;
    let content = json!({ "gist": "plain content", "n": 1 });
    let got = content_for(&cfg, &content, TEST_AAD).expect("package");
    match got {
        SyncContent::Plain(value) => assert_eq!(value, content, "plain content is carried as is"),
        SyncContent::Cipher(_) => panic!("the development mode must not seal"),
    }
}

/// The first entry of the list is the sealing key: the sealed content names
/// that key's identifier and opens under it, byte-for-byte.
#[test]
fn content_seals_with_the_first_entry_of_the_list() {
    let store = sqlite();
    let newest = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("feed-key-newest"));
    let older = SyncCipherKey::from_secret(&crate::credential_fixtures::secret("feed-key-older"));
    let cfg = cfg(
        "feed-sealed",
        &store,
        "127.0.0.1:1",
        vec![newest.clone(), older],
    );
    let content = json!({ "gist": "sealed content", "n": 2 });
    let got = content_for(&cfg, &content, TEST_AAD).expect("package");
    let cipher = match got {
        SyncContent::Cipher(cipher) => cipher,
        SyncContent::Plain(_) => panic!("a held key must seal"),
    };
    assert_eq!(
        cipher.key_id.as_deref(),
        Some(newest.key_id().as_str()),
        "the newest key, the first entry, is the sealing key"
    );
    let opened = newest
        .open(&cipher, TEST_AAD)
        .expect("open under the sealing key");
    assert_eq!(
        opened,
        serde_json::to_vec(&content).expect("serialize"),
        "the sealed bytes are the serialized content"
    );
}

/// The idempotency key and the wire stamp are two renderings of one text: the
/// key is hashed over the canonical stamp the envelope carries, so a peer
/// recomputes the id from `transaction_time` alone. An already-canonical stamp
/// keeps the id it always had; a row whose store text was not canonical has an
/// id that moves once, onto the canonical text, in the same push that moves its
/// stamp.
#[test]
fn the_idempotency_key_is_derived_from_the_canonical_wire_stamp() {
    use crate::client_feed::package::assoc_mutation;
    use crate::client_feed_package_fixtures::{
        BANK, MILLIS_STAMP_ISO, RELATIONSHIP, edge, millis_edge,
    };
    /// The stamp the shared edge fixture already carries.
    const EDGE_STAMP: &str = "2026-01-01T02:00:00Z";
    let cfg = crate::client_feed_package_fixtures::plain_cfg();
    let canonical = assoc_mutation(&cfg, BANK, &edge("e-id", RELATIONSHIP)).expect("package");
    assert_eq!(
        canonical.transaction_time, EDGE_STAMP,
        "an already-canonical stamp stays byte-identical on the wire"
    );
    assert_eq!(
        canonical.sync_mutation_id,
        "b57b4597267625b4ba1320e375186d147d3f4285f03fbe7fdf909586b636baa5",
        "the canonical-stamp edge keeps the id it always had"
    );
    let millis = assoc_mutation(&cfg, BANK, &millis_edge("e-id")).expect("package");
    assert_eq!(
        millis.transaction_time, MILLIS_STAMP_ISO,
        "the millis row's stamp moves to the canonical second the wire carries"
    );
    assert_eq!(
        millis.sync_mutation_id, "0bd79661e19b1b2c5ca5dcc6fa6f950dd3498772e4ee3e494c80f1a825d7c5db",
        "and its id moves with it, onto that same canonical text"
    );
}

/// A persona row whose stored stamp is epoch millis, the form the store itself
/// writes, leaves with a canonical stamp and an id hashed over that same text,
/// so the pair the relay stores is mutually derivable.
#[test]
fn a_persona_row_stamped_in_millis_carries_a_matching_id() {
    use crate::client_feed::package::persona_stable_mutation;
    use crate::client_feed_package_fixtures::{
        ATTR_NAME, BANK, MILLIS_STAMP, MILLIS_STAMP_ISO, plain_cfg, stable_entry,
    };
    let m = persona_stable_mutation(&plain_cfg(), BANK, MILLIS_STAMP, &stable_entry(ATTR_NAME))
        .expect("package the millis persona");
    assert_eq!(
        m.transaction_time, MILLIS_STAMP_ISO,
        "the wire carries the canonical second, not the store's millis text"
    );
    assert_eq!(
        m.sync_mutation_id,
        clio_store::sha256_hex(
            crate::keyed_roles::published_mutation_id_salt(),
            &format!("{ATTR_NAME}|{MILLIS_STAMP_ISO}|upsert")
        ),
        "the id is the hash of the identifier beside the wire's own stamp"
    );
    assert_eq!(
        m.sync_mutation_id, "16682970ea5fec6de66c6e36f71c76767e0063a7207a8fa0289626ad2afeecf5",
        "pinned as a literal, so neither side can drift unnoticed"
    );
}

/// A belief row whose stored stamp is a bare date, the third legend the column
/// carries, leaves the same way: the stamp places on the canonical clock and
/// the id is hashed over exactly that text.
#[test]
fn a_belief_row_stamped_with_a_bare_date_carries_a_matching_id() {
    use crate::client_feed::package::belief_mutation;
    use crate::client_feed_package_fixtures::{BANK, belief, plain_cfg};
    /// A bare date: no time, no zone, one legend the store column can hold.
    const BARE_DATE: &str = "2026-03-04";
    let m = belief_mutation(&plain_cfg(), BANK, BARE_DATE, &belief("b-1")).expect("package");
    assert_eq!(
        m.transaction_time, "2026-03-04T00:00:00Z",
        "a bare date places on the canonical clock at midnight"
    );
    assert_eq!(
        m.sync_mutation_id,
        clio_store::sha256_hex(
            crate::keyed_roles::published_mutation_id_salt(),
            "b-1|2026-03-04T00:00:00Z|upsert"
        ),
        "the id names the belief id beside the canonical wire stamp"
    );
    assert_eq!(
        m.sync_mutation_id, "658e07e56e86a89d7c27087156510266a0b40751c88447a869d262ab4ea0a7b8",
        "pinned as a literal, so neither side can drift unnoticed"
    );
}
