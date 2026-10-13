// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the four key roles one sync key plays, and
//! for the three relay-facing values derived under the three non-sealing
//! roles: the content digest a relay cannot confirm a guessed plaintext
//! against, the opaque record-identifier handle a relay cannot read a
//! caller-chosen string out of, and the idempotency key a relay cannot confirm
//! a guessed identifier against. Each role derives its own subkey from the sync
//! key under its own label, so no two roles share key material.
//!
//! ## Owns
//! - The role labels and the subkey derivation from a `SyncCipherKey`
//! - The keyed content digest, and its plaintext-mode fallback
//! - The opaque identifier handle, its width, and its preimage
//! - The keyed mutation idempotency key, and its plaintext-mode fallback
//! - Which record kinds carry a handle
//!
//! ## Does not own
//! - The AEAD seal and open, and the wire key identifier (`crypto`)
//! - Mutation packaging and its call sites (`client_feed_package`)
//! - Resolving a handle back to a real identifier on the pull path (`client`)
//!
//! ## Boundary
//!
//! Changes to what a role derives, at what width, or for which record kinds
//! should normally require changes here. Changes to the sealing key itself
//! should not require changes here.
//!
//! Keep this module focused on derived key roles and the values they
//! produce.

use crate::crypto::SyncCipherKey;
use crate::protocol::{
    ENTITY_KIND_ASSOC, ENTITY_KIND_FAILURE, ENTITY_KIND_PERSONA, ENTITY_KIND_TASK,
};
use clio_store::keyed_digest_hex;

/// Domain label of the role whose key seals content.
pub(crate) const ROLE_SEAL: &[u8] = b"clio-sync-role-seal-v1\0";

/// Domain label of the role whose key digests sealed content. Distinct from the
/// sealing label and from the handle label, so a value produced under one role
/// can never be confirmed under another.
pub(crate) const ROLE_DIGEST: &[u8] = b"clio-sync-role-content-digest-v1\0";

/// Domain label of the role whose key derives the opaque record-identifier
/// handle.
pub(crate) const ROLE_HANDLE: &[u8] = b"clio-sync-role-identifier-handle-v1\0";

/// Domain label of the role whose key derives the mutation idempotency key the
/// relay stores beside the row. Distinct from the sealing, digest, and handle
/// labels, so the idempotency key is never confirmable from a value another
/// role derived and never confirms one.
pub(crate) const ROLE_MUTATION_ID: &[u8] = b"clio-sync-role-mutation-id-v1\0";

/// Domain label inside the digest role, separating the content digest from any
/// other value that role might derive later.
const CONTENT_DIGEST_LABEL: &[u8] = b"content-digest";

/// Domain label inside the handle role, separating the record-identifier handle
/// from any other value that role might derive later.
const IDENTIFIER_HANDLE_LABEL: &[u8] = b"identifier-handle";

/// Domain label inside the mutation-id role, separating the idempotency key
/// from any other value that role might derive later.
const MUTATION_ID_LABEL: &[u8] = b"mutation-id";

/// Plaintext-mode content digest, unchanged from the pre-keyed build: an
/// unkeyed hash over a published domain string. A relay holding the journal can
/// confirm a guessed plaintext against it, which is exactly what the keyed form
/// below removes and exactly why a deployment with no cipher keeps it.
const PLAINTEXT_DIGEST_SALT: &[u8] = b"am-sync-hash";

/// Plaintext-mode idempotency key, unchanged from the pre-keyed build, over the
/// same published domain string and the same preimage. With no cipher there is
/// no key to derive it under, and nothing to hide: the row already carries the
/// real identifier in the clear.
const PLAINTEXT_MUTATION_ID_SALT: &[u8] = b"am-sync-mut";

/// Bytes of handle kept per record. Half of the hash output, which is the
/// floor the construction's own specification recommends, and the width this
/// codebase already chose for its other keyed blind index.
pub(crate) const HANDLE_BYTES: usize = 16;

/// The kinds whose business identifier carries a caller-chosen string, and so
/// travel as a handle when a cipher is configured. The remaining kinds
/// (`item`, `triple`, `belief`) carry machine-generated identifiers, which the
/// relay learns nothing new from, and `tombstone_meta` has no producer.
///
/// A handle is applied to the whole kind rather than per record shape, so the
/// two persona payload shapes (`stable` and `preference`) are covered by one
/// entry.
pub(crate) fn kind_uses_handle(kind: &str) -> bool {
    matches!(
        kind,
        ENTITY_KIND_PERSONA | ENTITY_KIND_TASK | ENTITY_KIND_FAILURE | ENTITY_KIND_ASSOC
    )
}

/// The sealed-content digest on the wire.
///
/// With no cipher this is byte-for-byte what a plaintext deployment has always
/// stored, so every existing apply-guard comparison against a stored row still
/// matches. With a cipher it is a keyed digest over the same serialized
/// plaintext bytes, under the digest role's own subkey, so a relay holding the
/// journal cannot hash a candidate plaintext and learn whether it is present.
///
/// The value is derived from the content and never from the identifier, and
/// never recomputed on the receiving side: both peers derive the same answer
/// from the same key, and the apply guard compares whatever arrived.
#[must_use]
pub fn content_digest(cfg_key: Option<&SyncCipherKey>, content: &str) -> String {
    let Some(key) = cfg_key else {
        return clio_store::sha256_hex(PLAINTEXT_DIGEST_SALT, content);
    };
    keyed_digest_hex(
        &role_subkey(key, ROLE_DIGEST),
        CONTENT_DIGEST_LABEL,
        content.as_bytes(),
        32,
    )
}

/// The opaque handle for one record's real identifier, or the identifier itself
/// when no cipher is configured.
///
/// The preimage names the kind and the bank alongside the identifier, so the
/// same identifier in two banks, and the same identifier used by two kinds,
/// produce different handles. Two handles colliding would merge two records
/// that must stay separate, so the width is half the hash output and the label
/// is a role of its own.
#[must_use]
pub fn identifier_handle(
    cfg_key: Option<&SyncCipherKey>,
    kind: &str,
    bank: &str,
    real_id: &str,
) -> String {
    let Some(key) = cfg_key else {
        return real_id.to_owned();
    };
    let preimage = format!("{kind}|{bank}|{real_id}");
    keyed_digest_hex(
        &role_subkey(key, ROLE_HANDLE),
        IDENTIFIER_HANDLE_LABEL,
        preimage.as_bytes(),
        HANDLE_BYTES,
    )
}

/// The stable idempotency key one logical mutation event carries, used by the
/// relay to recognise a redelivery.
///
/// With no cipher this is byte-for-byte what a plaintext deployment has always
/// stored, over the same published domain string. With a cipher it is a keyed
/// digest over the same preimage under the mutation-id role's own subkey. The
/// preimage names the **real** identifier, never the wire handle, so a cipher
/// cannot make two records that must stay separate share one key, and the
/// unkeyed form is never published next to an encrypted row where a relay
/// could hash a guessed identifier against it and confirm the guess.
#[must_use]
pub fn mutation_id(
    cfg_key: Option<&SyncCipherKey>,
    real_id: &str,
    stamp: &str,
    op: &str,
) -> String {
    let preimage = format!("{real_id}|{stamp}|{op}");
    let Some(key) = cfg_key else {
        return clio_store::sha256_hex(PLAINTEXT_MUTATION_ID_SALT, &preimage);
    };
    keyed_digest_hex(
        &role_subkey(key, ROLE_MUTATION_ID),
        MUTATION_ID_LABEL,
        preimage.as_bytes(),
        32,
    )
}

/// One role's subkey, derived from the sync key under that role's own label.
///
/// Every role gets its own value, so a peer holding the sync key derives all of
/// them and a peer without it derives none. The derivation is the shared
/// keyed-digest construction rather than a hash of a bare secret, which is what
/// the key-separation requirement asks for.
#[must_use]
pub fn role_subkey(key: &SyncCipherKey, role: &[u8]) -> Vec<u8> {
    clio_store::keyed_digest(key.key_bytes(), role, &[], 32)
}

/// The published plaintext-mode domain label, exposed for the tests across the
/// crate that pin the wire format. The label itself stays private.
#[cfg(test)]
pub(crate) fn published_digest_salt() -> &'static [u8] {
    PLAINTEXT_DIGEST_SALT
}

/// The published plaintext-mode idempotency label, exposed for the same tests.
#[cfg(test)]
pub(crate) fn published_mutation_id_salt() -> &'static [u8] {
    PLAINTEXT_MUTATION_ID_SALT
}

#[cfg(test)]
#[path = "keyed_roles_tests.rs"]
mod tests;
