// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing what the packaging helpers put in
//! the wire identifier column: that every handled kind puts an opaque handle
//! there instead of a caller-chosen string, that a machine-generated identifier
//! is left alone, that the handle is stable and distinct along the identifier,
//! bank, and kind axes, that it has the width and format the wire expects, and
//! that the idempotency key follows the real identifier rather than the handle
//! while staying unreadable to a relay holding only the journal.
//!
//! ## Owns
//! - Packaging-level assertions for the wire identifier and the mutation id
//!
//! ## Does not own
//! - The content digest (`client_feed_package_digest_tests`)
//! - What the relay can read off its stored bytes
//!   (`client_wire_blindness_tests`)
//! - Pull-path resolution (`client_resolve_tests`)
//! - The derivations themselves (`keyed_roles_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the wire identifier changes; they never change it.
//!
//! Keep this module focused on the packaged wire identifier.

use crate::client_feed::package::{
    assoc_mutation, belief_mutation, failure_mutation, item_mutation, persona_pref_mutation,
    persona_stable_mutation, task_mutation,
};
use crate::client_feed_package_fixtures::{
    ATTR_NAME, BANK, FAILURE_ID, RELATIONSHIP, STAMP, TASK_ID, belief, carrier, edge,
    failure_record, plain_cfg, preference, sealed_cfg, stable_entry, task_record,
};

/// Every handled kind puts a handle on the wire, so the identifier a relay
/// reads is not any of the caller-chosen strings.
#[test]
fn each_handled_kind_puts_a_handle_on_the_wire() {
    let c = sealed_cfg(&crate::credential_fixtures::secret("pkg-handle-key"));
    let persona =
        persona_stable_mutation(&c, BANK, STAMP, &stable_entry(ATTR_NAME)).expect("persona stable");
    assert_ne!(persona.entity_id, ATTR_NAME, "the attr key is hidden");
    let pref = persona_pref_mutation(&c, BANK, STAMP, &preference(ATTR_NAME)).expect("pref");
    assert_ne!(pref.entity_id, ATTR_NAME, "the preference key is hidden");

    let task = task_mutation(
        &c,
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task");
    assert_ne!(task.entity_id, TASK_ID, "the task id is hidden");

    let failure = failure_mutation(
        &c,
        BANK,
        STAMP,
        &failure_record(FAILURE_ID, TASK_ID),
        &carrier("car-fail"),
    )
    .expect("failure");
    assert_ne!(failure.entity_id, FAILURE_ID, "the failure id is hidden");

    let assoc = assoc_mutation(
        &c,
        BANK,
        &edge(
            &clio_store::explicit_id(BANK, "i-1", "i-2", RELATIONSHIP),
            RELATIONSHIP,
        ),
    )
    .expect("assoc");
    assert!(
        !assoc.entity_id.contains(RELATIONSHIP),
        "the relationship label is hidden: {}",
        assoc.entity_id
    );
}

/// The machine-generated kinds keep their identifier, because a relay learns
/// nothing from a value it could not have guessed a use for.
#[test]
fn machine_generated_identifiers_are_left_alone() {
    let c = sealed_cfg(&crate::credential_fixtures::secret("pkg-machine-key"));
    let item_m = item_mutation(
        &c,
        &crate::client_feed_package_fixtures::item("itm-1-2-3"),
        "upsert",
    )
    .expect("item");
    assert_eq!(item_m.entity_id, "itm-1-2-3", "an item id travels as it is");
    let belief_m = belief_mutation(&c, BANK, STAMP, &belief("bel-1")).expect("belief");
    assert_eq!(
        belief_m.entity_id, "bel-1",
        "a belief id travels as it is, through the same shared helper"
    );
}

/// With no cipher, every kind's wire identifier is the real one, so plaintext
/// mode is unchanged for every field.
#[test]
fn plaintext_mode_keeps_the_real_identifier() {
    let c = plain_cfg();
    let task = task_mutation(
        &c,
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task");
    assert_eq!(task.entity_id, TASK_ID, "the task id travels as it is");
    let persona =
        persona_stable_mutation(&c, BANK, STAMP, &stable_entry(ATTR_NAME)).expect("persona");
    assert_eq!(
        persona.entity_id, ATTR_NAME,
        "the attr key travels as it is"
    );
    let failure = failure_mutation(
        &c,
        BANK,
        STAMP,
        &failure_record(FAILURE_ID, TASK_ID),
        &carrier("car-fail"),
    )
    .expect("failure");
    assert_eq!(
        failure.entity_id, FAILURE_ID,
        "the failure id travels as is"
    );
}

/// Two records that must stay distinct get distinct handles along all three
/// axes: two identifiers, two banks, and two kinds. The same triple gives the
/// same handle, which is what makes idempotency work across peers.
#[test]
fn handles_differ_along_the_identifier_bank_and_kind_axes() {
    let c = sealed_cfg(&crate::credential_fixtures::secret("pkg-axes-key"));
    let first = task_mutation(
        &c,
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task one")
    .entity_id;
    let other_id = task_mutation(
        &c,
        BANK,
        STAMP,
        &task_record("synthetic-task-id-43", "task-row-2"),
        &carrier("car-task"),
    )
    .expect("task two")
    .entity_id;
    let other_bank = task_mutation(
        &c,
        "bh2",
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task other bank")
    .entity_id;
    let failure = failure_mutation(
        &c,
        BANK,
        STAMP,
        &failure_record(TASK_ID, TASK_ID),
        &carrier("car-fail"),
    )
    .expect("failure")
    .entity_id;
    assert_ne!(first, other_id, "two task ids");
    assert_ne!(first, other_bank, "the same task id in two banks");
    assert_ne!(first, failure, "the same identifier under two kinds");
    let again = task_mutation(
        &c,
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task again")
    .entity_id;
    assert_eq!(first, again, "stable for the same kind, bank and id");
}

/// Two relationship labels on one endpoint pair produce two distinct edge ids
/// and two distinct handles, so the records cannot merge.
#[test]
fn two_relationship_labels_on_one_pair_do_not_merge() {
    let c = sealed_cfg(&crate::credential_fixtures::secret("pkg-rel-key"));
    let id_a = clio_store::explicit_id(BANK, "i-1", "i-2", "rel_a");
    let id_b = clio_store::explicit_id(BANK, "i-1", "i-2", "rel_b");
    assert_ne!(id_a, id_b, "two labels give two edge ids");
    let first = assoc_mutation(&c, BANK, &edge(&id_a, "rel_a")).expect("edge a");
    let second = assoc_mutation(&c, BANK, &edge(&id_b, "rel_b")).expect("edge b");
    assert_ne!(
        first.entity_id, second.entity_id,
        "two labels on one endpoint pair must not share a handle"
    );
    assert_ne!(
        first.sync_mutation_id, second.sync_mutation_id,
        "and their idempotency keys differ, so neither is a redelivery"
    );
}

/// With no cipher the idempotency key is the plain unkeyed hash it has always
/// been, byte for byte what a plaintext deployment stored, so every existing
/// redelivery check against a stored row still matches.
#[test]
fn plaintext_mode_keeps_the_plain_mutation_id() {
    let plain_task = task_mutation(
        &plain_cfg(),
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("plain task");
    assert_eq!(
        plain_task.sync_mutation_id,
        clio_store::sha256_hex(
            b"am-sync-mut",
            &format!("{TASK_ID}|{}|upsert", task_record(TASK_ID, "task-row-1").id)
        ),
        "the unkeyed hash over the real identifier, stamp, and op"
    );
    let plain_persona = persona_pref_mutation(&plain_cfg(), BANK, STAMP, &preference(ATTR_NAME))
        .expect("plain pref");
    assert_eq!(
        plain_persona.sync_mutation_id,
        clio_store::sha256_hex(b"am-sync-mut", &format!("{ATTR_NAME}|{STAMP}|upsert")),
        "the shared helper's plaintext key is the same unkeyed hash"
    );
}

/// With a cipher the idempotency key is keyed under a role of its own, so a
/// relay holding it cannot hash a guessed identifier and confirm the guess
/// against the stored row.
#[test]
fn a_cipher_makes_the_mutation_id_unconfirmable_by_a_guess() {
    let mutid_key = crate::credential_fixtures::secret("pkg-mutid-blind");
    let sealed = task_mutation(
        &sealed_cfg(&mutid_key),
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("sealed task");
    let guess = clio_store::sha256_hex(
        crate::keyed_roles::PUBLISHED_MUTATION_ID_SALT,
        &format!("{TASK_ID}|{}|upsert", task_record(TASK_ID, "task-row-1").id),
    );
    assert_ne!(
        sealed.sync_mutation_id, guess,
        "the stored key is not the published unkeyed hash of the identifier"
    );
    let persona = persona_stable_mutation(
        &sealed_cfg(&mutid_key),
        BANK,
        STAMP,
        &stable_entry(ATTR_NAME),
    )
    .expect("sealed persona");
    assert_ne!(
        persona.sync_mutation_id,
        clio_store::sha256_hex(
            crate::keyed_roles::PUBLISHED_MUTATION_ID_SALT,
            &format!("{ATTR_NAME}|{STAMP}|upsert"),
        ),
        "nor is the shared helper's key the hash of the attribute name"
    );
}

/// The keyed idempotency key is still derived from the **real** identifier, not
/// from the wire handle: it stays stable for the same record and key, it differs
/// for two records that must stay separate, and a node without the key derives
/// nothing. That is what keeps a handle collision from becoming a merged record.
#[test]
fn the_keyed_mutation_id_still_follows_the_real_identifier() {
    let key = crate::credential_fixtures::secret("pkg-mutid-follow");
    let first = task_mutation(
        &sealed_cfg(&key),
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("first");
    let again = task_mutation(
        &sealed_cfg(&key),
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("again");
    let other_id = task_mutation(
        &sealed_cfg(&key),
        BANK,
        STAMP,
        &task_record("synthetic-task-id-43", "task-row-1"),
        &carrier("car-task"),
    )
    .expect("other id");
    let stranger = task_mutation(
        &sealed_cfg(&crate::credential_fixtures::secret("pkg-mutid-other-key")),
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("stranger");
    assert_eq!(
        first.sync_mutation_id, again.sync_mutation_id,
        "the same record and key give the same key"
    );
    assert_ne!(
        first.sync_mutation_id, other_id.sync_mutation_id,
        "two task ids that must stay separate get two keys"
    );
    assert_ne!(
        first.sync_mutation_id, stranger.sync_mutation_id,
        "a node without the key cannot reproduce it"
    );
    assert_eq!(
        first.entity_id, again.entity_id,
        "and the wire handle is stable alongside it"
    );
}

/// A handle is a fixed-width lowercase hex string with no letter prefix, so it
/// fits a plain text column and is compared with `=` rather than tokenised.
#[test]
fn a_handle_has_the_fixed_width_and_format_the_wire_expects() {
    let m = task_mutation(
        &sealed_cfg(&crate::credential_fixtures::secret("pkg-format-key")),
        BANK,
        STAMP,
        &task_record(TASK_ID, "task-row-1"),
        &carrier("car-task"),
    )
    .expect("task");
    assert_eq!(m.entity_id.len(), 32, "16 bytes rendered as hex");
    assert!(
        m.entity_id
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "lowercase hex, got {}",
        m.entity_id
    );
}

/// Two peers holding the same key derive the same handle from the same record,
/// and a node without the key cannot.
#[test]
fn two_peers_sharing_a_key_derive_the_same_handle() {
    let record = task_record(TASK_ID, "task-row-1");
    let peer_key = crate::credential_fixtures::secret("pkg-peer-key");
    let mine = task_mutation(
        &sealed_cfg(&peer_key),
        BANK,
        STAMP,
        &record,
        &carrier("car-task"),
    )
    .expect("mine");
    let theirs = task_mutation(
        &sealed_cfg(&peer_key),
        BANK,
        STAMP,
        &record,
        &carrier("car-task"),
    )
    .expect("theirs");
    let stranger = task_mutation(
        &sealed_cfg(&crate::credential_fixtures::secret("pkg-stranger-key")),
        BANK,
        STAMP,
        &record,
        &carrier("car-task"),
    )
    .expect("stranger");
    assert_eq!(
        mine.entity_id, theirs.entity_id,
        "peers sharing a key agree"
    );
    assert_ne!(
        mine.entity_id, stranger.entity_id,
        "a node without the key cannot derive it"
    );
}
