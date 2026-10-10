// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared test scaffolding the encryption-key
//! tests stand on: a fake OS secret store, temp paths for key files, and the
//! distinct key values a test needs.
//!
//! ## Owns
//! - [`FakeStore`], an in-memory [`SecretStore`] with call counting
//! - Temp key-file paths, seeded key files, and error unwrapping
//!
//! ## Does not own
//! - Any test (see `secret_key_tests` and `secret_key_report_tests`)
//! - The lifecycle implementation (see `secret_key`)
//!
//! ## Boundary
//!
//! This module exists so no test can reach the machine's real Keychain or Secret
//! Service: there is no constructor here that produces a production store.
//!
//! Keep this module focused on test scaffolding, with no tests of its own.
//!
//! # Known gaps
//!
//! Two things here are weaker than they should be, left deliberately rather than
//! fixed in passing:
//!
//! - The temp directories are unique per call and are never removed, so a test
//!   run leaves files under the system temp directory. `tempfile::tempdir()`
//!   would clean up on drop; adopting it means a new dev-dependency.
//! - `random_key` draws from the thread RNG rather than a fixed seed, so a failing
//!   assertion cannot be replayed from its own output. The keys only need to
//!   differ from each other, which they do.

use super::*;
use crate::secret_key_source::{self as secret_key_source, KeySource, write};
use crate::secret_key_store::NoKeystore;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// In-memory stand-in for the OS secret store, with call counting.
///
/// The counters are what make "did Clio write a second key?" answerable: a
/// second `set` is the failure that would make every memory unreadable.
#[derive(Default)]
pub(super) struct FakeStore {
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    entry: Option<Vec<u8>>,
    get_error: Option<String>,
    set_error: Option<String>,
    gets: usize,
    sets: usize,
}

impl FakeStore {
    /// Reachable and holding no entry: the state after a fresh install.
    pub(super) fn empty() -> Self {
        Self::default()
    }

    /// Reachable and already holding a key.
    pub(super) fn holding(bytes: &[u8]) -> Self {
        Self::with(|state| state.entry = Some(bytes.to_vec()))
    }

    /// Unreachable, as a headless machine with no Secret Service daemon is.
    pub(super) fn unreadable(message: &str) -> Self {
        Self::with(|state| state.get_error = Some(message.to_owned()))
    }

    /// Readable but not writable, as a locked store is.
    pub(super) fn refusing_writes(message: &str) -> Self {
        Self::with(|state| state.set_error = Some(message.to_owned()))
    }

    fn with(build: impl FnOnce(&mut FakeState)) -> Self {
        let store = Self::default();
        build(&mut store.lock());
        store
    }

    pub(super) fn entry(&self) -> Option<Vec<u8>> {
        self.lock().entry.clone()
    }

    pub(super) fn get_calls(&self) -> usize {
        self.lock().gets
    }

    pub(super) fn set_calls(&self) -> usize {
        self.lock().sets
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().expect("fake store mutex")
    }
}

impl SecretStore for FakeStore {
    fn get(&self) -> Result<Option<Vec<u8>>, AmError> {
        let mut state = self.lock();
        state.gets += 1;
        if let Some(message) = &state.get_error {
            return Err(unreachable_store(message));
        }
        Ok(state.entry.clone())
    }

    fn set(&self, bytes: &[u8]) -> Result<(), AmError> {
        let mut state = self.lock();
        state.sets += 1;
        if let Some(message) = &state.set_error {
            return Err(unreachable_store(message));
        }
        state.entry = Some(bytes.to_vec());
        Ok(())
    }

    fn delete(&self) -> Result<(), AmError> {
        self.lock().entry = None;
        Ok(())
    }
}

/// A keystore failure shaped like the ones `KeyringStore` reports, so a test can
/// say what went wrong and the message reads the way an operator sees it.
pub(super) fn unreachable_store(message: &str) -> AmError {
    AmError::new(ErrorCode::Internal, message.to_owned())
}

/// A temp path that is not inside an existing directory.
pub(super) fn tmp(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-secret-key-{name}-{nanos}"))
}

/// A key-file path in a fresh temp directory, parent created.
pub(super) fn key_file(name: &str) -> PathBuf {
    let dir = tmp(name);
    fs::create_dir_all(&dir).expect("temp dir");
    dir.join(KEY_FILE_NAME)
}

/// Record `source` as this machine's chosen store, beside `file`.
///
/// Every test that exercises a specific store has to say which one, because the
/// default is the file store and the keystore is now opt-in. A test that does not
/// record is testing the default, which is deliberate on its own.
pub(super) fn record(file: &std::path::Path, source: KeySource) {
    let marker = secret_key_source::marker_path(Some(file)).expect("a key file has a directory");
    write(Some(&marker), source).expect("the record is written");
}

/// A key-file path pointing at a directory, so reading it fails.
pub(super) fn unreadable_file() -> PathBuf {
    let dir = tmp("is-a-dir");
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A key-file path whose ancestor is a regular file, so creating it fails.
pub(super) fn blocked_key_file() -> PathBuf {
    let blocker = tmp("blocked");
    fs::write(&blocker, b"not a directory").expect("blocker");
    blocker.join("nested").join(KEY_FILE_NAME)
}

/// Injected environment lookup from key/value pairs.
pub(super) fn env(
    pairs: &[(&'static str, &'static str)],
) -> impl Fn(&str) -> Option<String> + 'static {
    let pairs = pairs.to_vec();
    move |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
    }
}

pub(super) fn write_key(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("seed key file");
}

/// A key that is not the one already stored, for tests that need two.
pub(super) fn random_key() -> [u8; KEY_LEN] {
    rand::random::<[u8; KEY_LEN]>()
}

/// A store that is not there at all: the key file is the only key.
///
/// The stand-in for the operating system's secret store when the operator sets
/// [`KEY_FILE_ONLY_ENV`]. It reports the store as reachable-and-empty and refuses
/// writes, which is what makes the existing lifecycle rules put the key in the
/// file without a single line knowing that the keystore was skipped.
pub(super) fn absent_keystore() -> NoKeystore {
    NoKeystore
}
