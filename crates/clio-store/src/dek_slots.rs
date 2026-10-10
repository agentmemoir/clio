// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the in-memory DEK slot model and the slot
//! operations over the local key-store database.
//!
//! ## Owns
//! - `KeySlot` and `DekMaterial`, the in-memory key state
//! - `SlotStore`: ensure / lookup / destroy over `local_kms_db`
//! - Conflict-safe key creation and the crypto-shred outcome
//!
//! ## Does not own
//! - The DEK provider trait and its crypto (see `dek`)
//! - The on-disk key-store format and rows (see `local_kms_db`)
//!
//! ## Boundary
//!
//! Changes to how slots are read, created, or shredded should normally require
//! changes here. Changes to the key-store file format should not require
//! changes here.
//!
//! Keep this module focused on the slot model and its operations.

use crate::local_kms_db::{KEY_LEN, KmsDb, PersistedSlot, decode_key, encode_key};
use clio_types::{AmError, ErrorCode};
use rand::RngCore;
use std::path::PathBuf;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Key bytes for one subject; zeroized when the slot is dropped.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct DekMaterial {
    pub(crate) bytes: [u8; KEY_LEN],
}

/// One subject's key state.
pub(crate) enum KeySlot {
    /// Live key at `version`.
    Active { version: u32, key: DekMaterial },
    /// Crypto-shredded subject; `version` records what was destroyed.
    Destroyed { version: u32 },
}

impl KeySlot {
    fn from_persisted(slot: PersistedSlot) -> Result<Self, AmError> {
        match slot.key_b64 {
            Some(encoded) => Ok(Self::Active {
                version: slot.version,
                key: DekMaterial {
                    bytes: decode_key(&encoded)?,
                },
            }),
            None => Ok(Self::Destroyed {
                version: slot.version,
            }),
        }
    }
}

/// DEK slot operations over the local key-store database.
pub(crate) struct SlotStore {
    db: KmsDb,
}

impl SlotStore {
    /// Open a process-local store (nothing on disk).
    ///
    /// # Errors
    /// The in-memory key store cannot be created.
    pub(crate) fn in_memory() -> Result<Self, AmError> {
        Ok(Self {
            db: KmsDb::in_memory()?,
        })
    }

    /// Open the file-backed store at `path`.
    ///
    /// # Errors
    /// The store cannot be opened, migrated, or has an unsupported version.
    pub(crate) fn open(path: PathBuf) -> Result<Self, AmError> {
        Ok(Self {
            db: KmsDb::open(path)?,
        })
    }

    /// Read a subject's slot; `None` when the subject is absent.
    ///
    /// # Errors
    /// The key store cannot be read.
    pub(crate) fn lookup(&mut self, subject_id: &str) -> Result<Option<KeySlot>, AmError> {
        self.db
            .get(subject_id)?
            .map(KeySlot::from_persisted)
            .transpose()
    }

    /// The subject's active key, creating and persisting a fresh one when the
    /// subject is new.
    ///
    /// Creation is conflict-safe: when another process stores a key for the
    /// same subject first, that key wins; when it shreds the subject first,
    /// this returns `ErasedSubject` instead of resurrecting it.
    ///
    /// # Errors
    /// The subject was shredded, or the key store cannot be read or written.
    pub(crate) fn ensure(&mut self, subject_id: &str) -> Result<(u32, [u8; KEY_LEN]), AmError> {
        if let Some(slot) = self.lookup(subject_id)? {
            return active_key(slot, subject_id);
        }
        let mut bytes = [0u8; KEY_LEN];
        rand::rng().fill_bytes(&mut bytes);
        self.db
            .insert_if_absent(subject_id, 1, &encode_key(&bytes))?;
        // The insert is conflict-safe, so a row always exists now: either ours
        // or the one a concurrent writer stored (or shredded) first.
        let stored = self
            .lookup(subject_id)?
            .expect("DEK slot present after conflict-safe insert");
        active_key(stored, subject_id)
    }

    /// Mark a subject as crypto-shredded. Irreversible.
    ///
    /// # Errors
    /// The key store cannot be written.
    pub(crate) fn destroy(&mut self, subject_id: &str) -> Result<(), AmError> {
        self.db.shred(subject_id)
    }
}

/// The active key of a live slot, or the shredded error.
fn active_key(slot: KeySlot, subject_id: &str) -> Result<(u32, [u8; KEY_LEN]), AmError> {
    match slot {
        KeySlot::Active { version, key } => Ok((version, key.bytes)),
        KeySlot::Destroyed { .. } => Err(destroyed_error(subject_id)),
    }
}

/// The shared crypto-shred error for a destroyed subject.
fn destroyed_error(subject_id: &str) -> AmError {
    AmError::new(
        ErrorCode::ErasedSubject,
        format!("DEK for subject `{subject_id}` has been destroyed"),
    )
}

#[cfg(test)]
#[path = "dek_slots_tests.rs"]
mod tests;
