// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for sealing and opening a triple's asserted
//! VALUE under the carrier item's data-subject DEK.
//!
//! ## Owns
//! - The `triples.object_ciphertext` seal/open pair
//! - The named error for a triple row whose value body is missing
//!
//! ## Does not own
//! - The plaintext `subject` + `predicate` identity key (bi-temporal
//!   structure, deliberately not sealed — see `sql/001_core.sql`)
//! - Triple SQL or row mapping (see `sqlite_triple`, `triple_map`)
//! - Erasure of triple rows (see `sqlite_erase_purge`, `postgres_erase`)
//!
//! ## Boundary
//!
//! Changes to how the asserted value is protected should normally require
//! changes here. Changes to the plaintext identity key should require a
//! requirement amendment, not a change here.

use crate::dek::{CiphertextEnvelope, DekProvider};
use clio_types::{AmError, ErrorCode};

/// The sealed halves of one triple write, plus its DEK reference.
///
/// Grouped so a write commit takes the whole sealed payload by name rather than
/// as a growing positional tail of ciphertext strings, and so the relationship
/// between them (one subject, one DEK) is visible at the call site.
pub(crate) struct TripleSeal<'a> {
    /// Carrier item snapshot/gist/context, sealed under the subject DEK.
    pub carrier: &'a str,
    /// The asserted VALUE, sealed under the same DEK.
    pub object: &'a str,
    /// Opaque DEK reference recorded in `subject_keys`.
    pub dek_ref: &'a str,
}

/// Seal a triple's asserted value under the carrier item's subject DEK.
///
/// The plaintext is the UTF-8 object value alone: `subject` and `predicate`
/// are the plaintext identity key and are not part of the sealed body.
pub(crate) fn seal_object(
    kms: &dyn DekProvider,
    subject_id: &str,
    object: &str,
) -> Result<String, AmError> {
    let envelope = kms.encrypt(subject_id, object.as_bytes())?;
    Ok(envelope.to_storage_string())
}

/// Open a sealed triple value.
///
/// # Errors
/// [`ErrorCode::UnreadableContent`] when the row has no sealed body, when the
/// body is not a readable envelope, or when the subject's DEK is gone. The
/// last case is the compliance path working as specified: after a
/// verified erasure the value is permanently unrecoverable, so callers that
/// must keep serving reads treat it as unreadable rather than failing.
pub(crate) fn open_object(kms: &dyn DekProvider, cipher: &str) -> Result<String, AmError> {
    if cipher.is_empty() {
        return Err(missing_object());
    }
    let envelope = CiphertextEnvelope::from_storage_string(cipher)?;
    let bytes = kms.decrypt(&envelope)?;
    String::from_utf8(bytes).map_err(|_| unreadable_object("triple value is not valid UTF-8"))
}

/// Open a sealed triple value read from a nullable column.
///
/// A `NULL` body means a legacy-format row restored through the opaque
/// ciphertext_backup route without a sealed body, which
/// is a store-level problem. It maps to the same [`ErrorCode::UnreadableContent`]
/// as an empty body rather than to an empty value, so a caller never renders a
/// real assertion as a blank one.
pub(crate) fn open_optional_object(
    kms: &dyn DekProvider,
    cipher: Option<&str>,
) -> Result<String, AmError> {
    open_object(kms, cipher.unwrap_or_default())
}

/// The named error for a triple row whose sealed value body is absent.
///
/// A fresh row always has one. A NULL or empty body is a legacy-format row
/// restored through the opaque ciphertext_backup route without a sealed body,
/// which is a store-level problem and
/// not a caller mistake.
pub(crate) fn missing_object() -> AmError {
    unreadable_object("triple row missing object_ciphertext")
}

/// One skippable classification for every "this value is not readable" case.
fn unreadable_object(detail: &str) -> AmError {
    AmError::new(ErrorCode::UnreadableContent, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dek::LocalDevKms;

    #[test]
    fn value_is_sealed_under_the_subject_and_opens_back() {
        let kms = LocalDevKms::new();
        let cipher = seal_object(&kms, "alice", "host:db.internal:5432").unwrap();
        assert!(!cipher.contains("db.internal"));
        assert_eq!(open_object(&kms, &cipher).unwrap(), "host:db.internal:5432");
    }

    #[test]
    fn a_tampered_value_body_does_not_authenticate() {
        let kms = LocalDevKms::new();
        let cipher = seal_object(&kms, "alice", "hunter2").unwrap();
        let mut envelope = CiphertextEnvelope::from_storage_string(&cipher).unwrap();
        // Flip one base64 character of the ciphertext.
        let first = envelope.ciphertext_b64.remove(0);
        envelope
            .ciphertext_b64
            .insert(0, if first == 'A' { 'B' } else { 'A' });
        let err = open_object(&kms, &envelope.to_storage_string()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
        assert!(crate::is_unreadable_content(&err), "{err:?}");
    }

    #[test]
    fn a_shredded_subject_leaves_the_value_unrecoverable() {
        let kms = LocalDevKms::new();
        let cipher = seal_object(&kms, "alice", "hunter2").unwrap();
        kms.destroy_dek("alice").unwrap();
        let err = open_object(&kms, &cipher).unwrap_err();
        assert_eq!(err.code, ErrorCode::ErasedSubject);
    }

    #[test]
    fn an_absent_or_malformed_body_is_unreadable_content() {
        let kms = LocalDevKms::new();
        assert_eq!(
            open_object(&kms, "").unwrap_err().code,
            ErrorCode::UnreadableContent
        );
        assert_eq!(
            open_object(&kms, "not-an-envelope").unwrap_err().code,
            ErrorCode::UnreadableContent
        );
        let non_utf8 = kms
            .encrypt("alice", &[0xff, 0xfe])
            .unwrap()
            .to_storage_string();
        assert_eq!(
            open_object(&kms, &non_utf8).unwrap_err().code,
            ErrorCode::UnreadableContent
        );
    }
}
