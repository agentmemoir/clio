// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the local dev-KMS key store: the SQLite
//! schema, connection handling, format-version gate, and point row operations.
//!
//! ## Owns
//! - `PersistedSlot` (subject id, version, optional base64 key)
//! - The `KmsDb` connection modes (in-memory and lazily opened file)
//! - The schema, pragmas, format-version gate, and `get`/`insert`/`shred`
//! - The plaintext boundary of `KmsDb`
//!
//! ## Does not own
//! - Store creation and installation (see `local_kms_migrate`)
//! - The v2 stored value shape (see `kms_migrate`)
//! - The DEK provider and its crypto (see `dek`)
//! - The in-memory slot model (see `dek_slots`)
//! - Choosing the key-store path (see `clio-config`'s `kms_path`)
//!
//! ## Boundary
//!
//! Changes to the SQLite schema or row operations should normally require
//! changes here. Changes to the stored value shape should require changes in
//! `kms_migrate` and not here. Changes to store creation or migration should not
//! require changes here.
//!
//! Keep this module focused on the key-store schema and rows.
//!
//! # `KmsDb` is the plaintext boundary
//!
//! `insert_if_absent` takes a plaintext base64 key and `get` returns one. A
//! file-backed store seals on the way in and opens on the way out, so every
//! caller above this module — and every test — is unaffected by the envelope,
//! and the only place that knows about it is `kms_migrate`. The opener is
//! file-only: a memory row is returned exactly as stored, without it.
//!
//! An in-memory store does neither. Its keys never reach a disk, so wrapping
//! them would protect nothing and would make every process-local key store
//! depend on the Clio encryption key existing.

use crate::kms_migrate::{unwrap_key_b64, wrap_key_b64};
use crate::local_kms_migrate::{create_parent_dir, install_empty_store, tighten_permissions};
pub(crate) use crate::local_kms_schema::{
    check_format_version, init_schema, insert_slot_if_absent, seed_format_version,
};
use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

/// DEK length in bytes.
pub(crate) const KEY_LEN: usize = 32;

/// SQLite file magic.
pub(crate) const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// One persisted DEK slot. `key_b64: None` marks a crypto-shredded subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistedSlot {
    /// Data-subject key this slot belongs to.
    pub(crate) subject_id: String,
    /// DEK version for this subject.
    pub(crate) version: u32,
    /// Base64 key bytes, or `None` when the DEK was destroyed.
    pub(crate) key_b64: Option<String>,
}

/// Where the store's rows live.
enum Backing {
    /// Process-local keys; nothing on disk.
    Memory(Connection),
    /// A SQLite file, opened lazily so a read-only run creates nothing.
    File {
        path: PathBuf,
        conn: Option<Connection>,
    },
}

/// The local dev-KMS key store.
pub(crate) struct KmsDb {
    backing: Backing,
    /// The process-wide registration of this file-backed key-store handle, so
    /// a restore can prove the live key store is closed before it is replaced.
    /// `None` for an in-memory store, which has no file. Held for its `Drop`;
    /// nothing reads it.
    #[allow(dead_code)]
    handle: Option<crate::open_handles::OpenHandle>,
}

impl KmsDb {
    /// Open an in-memory store (process-local keys).
    ///
    /// # Errors
    /// The in-memory schema cannot be created.
    pub(crate) fn in_memory() -> Result<Self, AmError> {
        let conn = Connection::open_in_memory().am("open in-memory key store")?;
        init_schema(&conn)?;
        seed_format_version(&conn)?;
        check_format_version(&conn)?;
        Ok(Self {
            backing: Backing::Memory(conn),
            handle: None,
        })
    }

    /// Open the file-backed store at `path`.
    ///
    /// A missing file is not created until the first write.
    ///
    /// # Errors
    /// The store exists but cannot be opened, or has an unsupported format
    /// version.
    pub(crate) fn open(path: PathBuf) -> Result<Self, AmError> {
        let conn = open_existing(&path)?;
        let handle = crate::open_handles::register(&path);
        Ok(Self {
            backing: Backing::File { path, conn },
            handle: Some(handle),
        })
    }

    /// Read one subject's slot; `None` when the subject (or file) is absent.
    ///
    /// # Errors
    /// The store file appeared but cannot be opened or read.
    pub(crate) fn get(&mut self, subject_id: &str) -> Result<Option<PersistedSlot>, AmError> {
        let file_backed = self.is_file_backed();
        let Some(conn) = self.connection()? else {
            return Ok(None);
        };
        let slot = conn
            .query_row(
                "SELECT version, key_b64 FROM slots WHERE subject_id = ?1",
                params![subject_id],
                |row| {
                    Ok(PersistedSlot {
                        subject_id: subject_id.to_owned(),
                        version: row.get(0)?,
                        key_b64: row.get(1)?,
                    })
                },
            )
            .optional()
            .am("read DEK slot")?;
        // The backing decides the read: a file row unwraps through the strict
        // opener, a memory row comes back exactly as stored, because it was
        // never wrapped and the opener would refuse it.
        match slot {
            Some(slot) if file_backed => slot.into_plaintext().map(Some),
            other => Ok(other),
        }
    }

    /// How many data-subject slots this store holds; `0` when the store file is
    /// absent. The reserved lexical blind-index slot names key infrastructure,
    /// not a data subject, so it is not counted (see
    /// [`crate::dek::BLIND_INDEX_KEY_SUBJECT`]): a one-subject store must not
    /// report two subjects to a backup operator.
    ///
    /// # Errors
    /// The store file appeared but cannot be opened or read.
    pub(crate) fn subject_count(&mut self) -> Result<u64, AmError> {
        let Some(conn) = self.connection()? else {
            return Ok(0);
        };
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM slots WHERE subject_id != ?1",
                params![crate::dek::BLIND_INDEX_KEY_SUBJECT],
                |row| row.get(0),
            )
            .am("count DEK slots")?;
        Ok(u64::try_from(count.max(0)).unwrap_or(0))
    }

    /// Insert a subject's key unless another writer got there first.
    ///
    /// `key_b64` is plaintext. A file-backed store seals it before the row is
    /// written; an in-memory store keeps it as it is, because nothing it holds
    /// ever reaches a disk.
    ///
    /// # Errors
    /// The store cannot be created or written, or the key cannot be sealed.
    pub(crate) fn insert_if_absent(
        &mut self,
        subject_id: &str,
        version: u32,
        key_b64: &str,
    ) -> Result<(), AmError> {
        let sealed = self
            .is_file_backed()
            .then(|| wrap_key_b64(key_b64))
            .transpose()?;
        let conn = self.connection_creating()?;
        insert_slot_if_absent(
            conn,
            subject_id,
            version,
            sealed.as_deref().unwrap_or(key_b64),
        )
    }

    /// Record a crypto-shred. The row's version is kept and `key_b64` becomes
    /// NULL; a shredded subject is never resurrected.
    ///
    /// # Errors
    /// The store cannot be created or written.
    pub(crate) fn shred(&mut self, subject_id: &str) -> Result<(), AmError> {
        let conn = self.connection_creating()?;
        shred_slot(conn, subject_id)
    }

    /// True when the slots live in a file, which is what decides whether the
    /// Clio encryption key is involved at all.
    fn is_file_backed(&self) -> bool {
        matches!(self.backing, Backing::File { .. })
    }

    /// The connection, opening the store file when another process created it.
    fn connection(&mut self) -> Result<Option<&Connection>, AmError> {
        match &mut self.backing {
            Backing::Memory(conn) => Ok(Some(conn)),
            Backing::File { path, conn } => {
                if conn.is_none() {
                    *conn = open_existing(path)?;
                }
                Ok(conn.as_ref())
            }
        }
    }

    /// The connection, creating the store file (and its parent) when missing.
    fn connection_creating(&mut self) -> Result<&Connection, AmError> {
        match &mut self.backing {
            Backing::Memory(conn) => Ok(conn),
            Backing::File { path, conn } => {
                if conn.is_none() {
                    *conn = Some(open_or_create(path)?);
                }
                Ok(conn.as_ref().expect("key store connection opened"))
            }
        }
    }
}

impl PersistedSlot {
    /// The row as a file-backed caller above this module sees it: `key_b64` is
    /// the plaintext base64 key, or `None` for a crypto-shredded subject.
    ///
    /// Only a file-backed store calls this. A memory row was never wrapped, so
    /// the strict opener would refuse it, and `get` returns such a row as is.
    ///
    /// # Errors
    /// The stored envelope is malformed, is not sealed at all, or was sealed
    /// under a different Clio encryption key.
    fn into_plaintext(self) -> Result<Self, AmError> {
        let Some(stored) = self.key_b64.as_deref() else {
            return Ok(self);
        };
        Ok(Self {
            key_b64: Some(unwrap_key_b64(stored)?),
            ..self
        })
    }
}

/// Open an existing store; `None` when it is absent.
fn open_existing(path: &Path) -> Result<Option<Connection>, AmError> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(open_ready(path)?))
}

/// Open or create the store, creating parent directories when needed.
fn open_or_create(path: &Path) -> Result<Connection, AmError> {
    create_parent_dir(path)?;
    if !path.exists() {
        install_empty_store(path)?;
    }
    open_ready(path)
}

/// Open a store that exists: initialize the schema, tighten permissions, and
/// check the format version.
fn open_ready(path: &Path) -> Result<Connection, AmError> {
    let conn = Connection::open(path).am("open key store")?;
    init_schema(&conn)?;
    tighten_permissions(path)?;
    check_format_version(&conn)?;
    Ok(conn)
}

/// Record a crypto-shred, keeping the row's version and never resurrecting it.
fn shred_slot(conn: &Connection, subject_id: &str) -> Result<(), AmError> {
    conn.execute(
        "INSERT INTO slots (subject_id, version, key_b64) VALUES (?1, 1, NULL)
         ON CONFLICT(subject_id) DO UPDATE SET key_b64 = NULL",
        params![subject_id],
    )
    .am("shred DEK slot")?;
    Ok(())
}

/// Encode key bytes for storage.
pub(crate) fn encode_key(bytes: &[u8; KEY_LEN]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

/// Decode and length-check a stored key.
pub(crate) fn decode_key(encoded: &str) -> Result<[u8; KEY_LEN], AmError> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .map_err(|_| AmError::new(ErrorCode::Internal, "invalid DEK encoding in store"))?;
    bytes
        .try_into()
        .map_err(|_| AmError::new(ErrorCode::Internal, "invalid DEK length in store"))
}

#[cfg(test)]
#[path = "local_kms_db_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "local_kms_db_envelope_tests.rs"]
mod local_kms_db_envelope_tests;
