// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the create/verify/inventory test fixtures
//! every one of those suites builds on: a scratch directory, a staged payload,
//! a key-store file, and a ready-to-run `CreateRequest`.
//!
//! ## Owns
//! - The scratch-directory guard and the payload / key-store byte fixtures
//! - `request` and `create` helpers that fill in a valid request
//!
//! ## Does not own
//! - Any assertion: every fixture is data, never a test of its own
//! - Create, verify, or inventory cases (see the sibling `*_tests.rs` files)
//!
//! ## Boundary
//!
//! Changes to how the shared fixtures are built should normally require
//! changes here. Adding a case to a sibling test module should not.

use crate::create::{CreateRequest, KeySource};
use crate::staging::{self, StagedFile};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so `Scratch::new` never returns a
/// directory another call already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// SQLite file magic, for fixtures that must look like a real file.
pub(crate) const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// A scratch directory removed when the guard drops.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(tag: &str) -> Self {
        // The counter is what makes this distinct: the process id is constant
        // inside one process, so clock-plus-pid alone repeats whenever two calls
        // land in the same tick, and two tests then share one directory.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "clio-backup-fx-{tag}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Payload bytes that look like a small SQLite store.
pub(crate) fn payload_bytes(seed: u8) -> Vec<u8> {
    let mut bytes = SQLITE_MAGIC.to_vec();
    bytes.extend((0..600u32).map(|index| (index as u8).wrapping_add(seed)));
    bytes
}

/// Stage a payload member in `dir` and return it measured.
pub(crate) fn staged_payload(dir: &Path, seed: u8) -> StagedFile {
    let source = dir.join("payload-source.bin");
    std::fs::write(&source, payload_bytes(seed)).expect("seed the payload source");
    staging::stage_copy(&source, dir, "payload").expect("stage the payload")
}

/// Write a key-store file in `dir` and return a `KeySource` for it.
pub(crate) fn key_source(dir: &Path, subjects: u64) -> KeySource {
    let path = dir.join("keys.kms.json");
    let mut bytes = SQLITE_MAGIC.to_vec();
    bytes.extend(std::iter::repeat_n(0x5Au8, 128));
    std::fs::write(&path, bytes).expect("write the key store");
    KeySource { path, subjects }
}

/// A valid create request over `payload`, with no key member.
pub(crate) fn request(dir: &Path, payload: StagedFile) -> CreateRequest {
    CreateRequest {
        backup_dir: dir.to_path_buf(),
        slug: "clio".to_owned(),
        backend: "sqlite".to_owned(),
        database: "clio".to_owned(),
        store_schema: None,
        schema_version: Some("1".to_owned()),
        payload,
        payload_member: "store.db".to_owned(),
        additional_members: Vec::new(),
        method: "vacuum-into".to_owned(),
        consistency: "point-in-time".to_owned(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 42,
        counts_complete: true,
        keys: None,
        label: None,
        note: None,
        tool_version: "clio 0.1.0".to_owned(),
    }
}

/// Every file in `dir`, sorted, so a test can assert nothing was left behind.
pub(crate) fn dir_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the scratch directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The NUL padding that follows a member of these bytes, up to the 512-byte
/// multiple a ustar member needs.
pub(crate) fn pad(bytes: &[u8]) -> Vec<u8> {
    const BLOCK: usize = 512;
    vec![0u8; (BLOCK - bytes.len() % BLOCK) % BLOCK]
}

/// Whether a file name ends in `extension`, compared through `Path` so the
/// check is not a raw string test on a case-sensitive host.
pub(crate) fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|found| found == extension)
}

/// The names in `dir` that end in `extension`, sorted.
pub(crate) fn names_with_extension(dir: &Path, extension: &str) -> Vec<String> {
    dir_entries(dir)
        .into_iter()
        .filter(|name| has_extension(name, extension))
        .collect()
}

/// Whether the mode bits stop this process writing into `dir`.
///
/// Root, and any process holding `CAP_DAC_OVERRIDE`, writes into a directory
/// whatever its mode, so a test that asserts a write refusal cannot observe one
/// there. The probe asks the question directly instead of reading the uid, and
/// it removes the file it created when the write is allowed.
#[cfg(unix)]
pub(crate) fn write_is_refused(dir: &Path) -> bool {
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(_) => true,
    }
}
