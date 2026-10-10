// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one envelope format that turns a secret
//! into text nobody can read back out of a file: sealing a value under the Clio
//! encryption key, and opening it again.
//!
//! ## Owns
//! - The `sealed:v1:` envelope, its nonce, and its authenticated context
//! - Which config paths are sealed, as an explicit list
//! - The strict read rule: only a sealed envelope opens, and a value with no
//!   prefix is refused
//!
//! ## Does not own
//! - The Clio encryption key's own lifecycle (see `secret_key`)
//! - Reading the operating system's secret store (see `secret_key_store`)
//! - Masking values for display (see `secret`)
//! - Writing the sealed values to disk (see `config`'s `persist`)
//! - Wrapping the per-subject keys in the key-store file (`clio-store`'s
//!   `kms_migrate` reuses this envelope rather than inventing a second one)
//!
//! ## Boundary
//!
//! Changes to the envelope format or to the sealed-path list should normally
//! require changes here. Changes to where the key comes from should not require
//! changes here, because `seal` and `open` both go through one key lookup.
//!
//! Keep this module focused on the envelope and the path list.
//!
//! # The envelope
//!
//! `sealed:v1:` followed by standard base64 of `nonce(12) || ciphertext`. One
//! JSON string, so the deployment overlay stays a flat map and
//! `deployment_field` still returns a string.
//!
//! # Why a constant authenticated context, not the config path
//!
//! The authenticated context is the constant `clio-secret-seal-v1`, not the
//! dot-path. Moving a secret from one path to another is a real future need, and
//! binding the path would turn a readable secret into a permanent error the
//! moment it moved.
//!
//! # Why a blank value is sealed too
//!
//! The empty string is sealed like anything else, and the envelope still opens
//! to `""`. A stored blank API key must not be distinguishable from a real one
//! by looking at the file.
//!
//! # The key
//!
//! `seal` and `open` load the key lazily through `secret_key::load_or_create`
//! and cache it for the process, because one save seals several values and the
//! operating system's secret store is slow enough to notice. A key that cannot
//! be loaded is an error, never a reason to store plaintext: falling back would
//! turn a locked keystore into silent exposure.

use crate::secret_key::{KEY_LEN, load_or_create};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use clio_types::{AmError, ErrorCode};
use rand::RngCore;
use std::sync::{Mutex, OnceLock};
use zeroize::Zeroizing;

/// Marker every sealed value starts with, carrying the envelope version.
pub const SEALED_PREFIX: &str = "sealed:v1:";

/// Authenticated context, constant so a value can move between config paths.
const AAD: &[u8] = b"clio-secret-seal-v1";

/// Nonce length in bytes, fixed by the cipher.
const NONCE_LEN: usize = 12;

/// Config paths whose values are sealed on disk, matched in full.
const SEALED_PATHS: &[&str] = &["store.database_url"];

/// Config path prefixes whose values are sealed on disk.
const SEALED_PREFIXES: &[&str] = &["credentials."];

/// The process-wide key cache.
///
/// Only a success is cached. A keystore that was locked during the first save
/// must not pin the failure, because the next command, once the operator has
/// unlocked it, has to be able to load the key.
static CACHED_KEY: OnceLock<Mutex<Option<Zeroizing<[u8; KEY_LEN]>>>> = OnceLock::new();

#[cfg(any(test, feature = "test-key-injection"))]
thread_local! {
    /// A key installed for the running thread only. See [`test_key`].
    static THREAD_KEY: std::cell::RefCell<Option<Zeroizing<[u8; KEY_LEN]>>> =
        const { std::cell::RefCell::new(None) };
}

/// True when a stored value is a sealed envelope.
///
/// This is what decides whether [`open`] accepts a value at all. It looks at
/// the value, never at the path, so a secret that moved keeps opening.
#[must_use]
pub fn is_sealed(value: &str) -> bool {
    value.starts_with(SEALED_PREFIX)
}

/// True when a config path's value is sealed on disk.
///
/// An explicit list, deliberately not the masking rule. Masking answers "should
/// a human not see this", which is a wider question: `history.lesson_max_tokens`
/// contains `token`, `injection.budget_tokens` contains it too, and both are
/// numbers a user sets. Sealing answers "would a readable copy of this file be a
/// disclosure", and a knob is not one. The two lists are kept apart on purpose.
#[must_use]
pub fn is_sealed_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    SEALED_PATHS.iter().any(|exact| lower == *exact)
        || SEALED_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
}

/// Seal a plaintext value under the Clio encryption key.
///
/// A value that is already sealed is returned unchanged, so saving the same
/// overlay twice never seals it twice.
///
/// # Errors
/// The key cannot be loaded, or the cipher refuses the input.
pub fn seal(plain: &str) -> Result<String, AmError> {
    if is_sealed(plain) {
        return Ok(plain.to_owned());
    }
    let key = load_key()?;
    seal_with_key(plain, &key)
}

/// Open a sealed envelope.
///
/// Only a value carrying the `sealed:v1:` prefix opens. A value without it is
/// refused: the caller above the envelope decides what a raw value means, so
/// the opener can never be the step that lets plaintext through by accident.
/// Legacy plaintext becomes sealed on the next save, not on the read.
///
/// # Errors
/// The value is not sealed at all, or the envelope is malformed, was sealed
/// under a different key, or has been altered since. The key cannot be loaded.
pub fn open(stored: &str) -> Result<String, AmError> {
    if !is_sealed(stored) {
        // Refused before the key is touched: a raw value needs no key, and a
        // load here could create one for an input that can never open.
        return Err(cipher_failure("open a value that is not sealed"));
    }
    let key = load_key()?;
    open_with_key(stored, &key)
}

/// Seal under an explicit key, bypassing the process cache.
///
/// # Errors
/// The cipher refuses the input, which for this cipher and key size it does not.
pub fn seal_with_key(plain: &str, key: &[u8; KEY_LEN]) -> Result<String, AmError> {
    if is_sealed(plain) {
        return Ok(plain.to_owned());
    }
    let mut nonce = [0_u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain.as_bytes(),
                aad: AAD,
            },
        )
        .map_err(|_| cipher_failure("seal"))?;
    let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&ciphertext);
    Ok(format!(
        "{SEALED_PREFIX}{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, sealed)
    ))
}

/// Open an envelope under an explicit key, bypassing the process cache.
///
/// # Errors
/// The value is not sealed at all, the envelope is truncated, is not base64, or
/// fails authentication. The message never repeats the stored text.
pub fn open_with_key(stored: &str, key: &[u8; KEY_LEN]) -> Result<String, AmError> {
    let body = stored
        .strip_prefix(SEALED_PREFIX)
        .ok_or_else(|| cipher_failure("open a value that is not sealed"))?;
    let sealed = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, body)
        .map_err(|_| cipher_failure("open a value whose envelope is not base64"))?;
    if sealed.len() <= NONCE_LEN {
        return Err(cipher_failure("open a value whose envelope is truncated"));
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let cipher = ChaCha20Poly1305::new(key.into());
    let plain = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: AAD,
            },
        )
        .map_err(|_| {
            cipher_failure(
                "open a value that fails authentication: its envelope was altered, or it was \
                 sealed under a different Clio encryption key",
            )
        })?;
    String::from_utf8(plain)
        .map_err(|_| cipher_failure("open a value whose plaintext is not valid UTF-8"))
}

/// One message shape for every envelope failure.
///
/// A caller that cannot read a stored secret has to be told which of the three
/// things went wrong, and told nothing about the value itself. The three are a
/// malformed envelope, an altered envelope, and a different key, and only the
/// caller knows which of those it can do anything about.
fn cipher_failure(what: &str) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "cannot {what}. Clio did not write a replacement value, because one sealed under a \
                 different key would be a different secret"
        ),
    )
}

/// The Clio encryption key for this process: an installed test key, else the
/// cached load, else a fresh load through the key's own lifecycle rules.
fn load_key() -> Result<Zeroizing<[u8; KEY_LEN]>, AmError> {
    #[cfg(any(test, feature = "test-key-injection"))]
    if let Some(key) = test_key::thread_key() {
        return Ok(key);
    }
    let cell = CACHED_KEY.get_or_init(|| Mutex::new(None));
    let mut cached = cell.lock().expect("secret seal key cache mutex");
    if let Some(key) = cached.as_ref() {
        return Ok(key.clone());
    }
    let loaded = load_or_create()?;
    let key = Zeroizing::new(*loaded.expose());
    *cached = Some(key.clone());
    Ok(key)
}

/// Test-only key injection.
///
/// This exists because a test cannot be allowed to reach the machine's real key
/// store, and because a test that needs a key it chose cannot share the process
/// cache with a test that needs a different one. It is compiled in under
/// `cfg(test)` and behind the `test-key-injection` feature, which only
/// dev-dependencies turn on, so nothing in a shipped binary can make a running
/// Clio seal under a key its operator did not create.
#[cfg(any(test, feature = "test-key-injection"))]
pub mod test_key {
    use super::{CACHED_KEY, KEY_LEN, THREAD_KEY};
    use std::sync::Mutex;
    use zeroize::Zeroizing;

    /// Install a key for every thread in this process. The first call wins.
    ///
    /// # Panics
    /// If another thread panicked while holding the key cache, which means a
    /// process that has already lost its key handling and cannot be trusted with
    /// one.
    pub fn install_process_key(key: [u8; KEY_LEN]) {
        let cell = CACHED_KEY.get_or_init(|| Mutex::new(None));
        let mut cached = cell.lock().expect("secret seal key cache mutex");
        cached.get_or_insert_with(|| Zeroizing::new(key));
    }

    /// Install a key for the running thread, in preference to the process key.
    ///
    /// The key is dropped when the thread ends, so one test cannot leak its key
    /// into the next test on a reused thread.
    pub fn use_key(key: [u8; KEY_LEN]) {
        THREAD_KEY.with(|slot| *slot.borrow_mut() = Some(Zeroizing::new(key)));
    }

    /// Forget the running thread's key, so it falls back to the process key.
    pub fn clear_thread_key() {
        THREAD_KEY.with(|slot| *slot.borrow_mut() = None);
    }

    /// The running thread's key, when one is installed.
    pub(super) fn thread_key() -> Option<Zeroizing<[u8; KEY_LEN]>> {
        THREAD_KEY.with(|slot| slot.borrow().clone())
    }
}

#[cfg(test)]
#[path = "secret_seal_tests.rs"]
mod tests;
