// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the Clio encryption key in the
//! operating system's secret store, and for describing that store in messages
//! an operator reads.
//!
//! ## Owns
//! - The [`SecretStore`] seam every keystore read and write goes through
//! - [`KeyringStore`], the production `keyring` wrapper and its entry seam
//! - [`NoKeystore`], the store that does not exist, for a key file that is the
//!   only key
//! - The platform name of the store used in operator-facing messages
//! - Keeping store contents out of error text
//!
//! ## Does not own
//! - Which store wins when both hold a key (see `secret_key`)
//! - The key file, its permissions, and its length check (see `secret_key`)
//! - Deciding whether the keystore is consulted at all (see `secret_key`)
//! - What the key protects (see the sealing stage)
//!
//! ## Boundary
//!
//! Changes to the OS secret store, its entry name, or the `keyring` features
//! should normally require changes here. Changes to key precedence or to the
//! key file should not require changes here.
//!
//! Keep this module focused on the operating-system secret store.

// The `keyring` dependency is a single unconditional workspace entry with the
// three native features (`apple-native`, `windows-native`,
// `linux-native-sync-persistent`) plus `vendored`. keyring gates its own backing
// crate per target, so this resolves to the Keychain on macOS, Credential
// Manager on Windows, and the kernel-keyring-backed persistent Secret Service
// on Linux without a target-conditional dependency here.
//
// `vendored` matters on Linux and only there: the Secret Service backend reaches
// the session bus through `libdbus`, whose default build shells out to
// `pkg-config` for a system `libdbus-1`. `vendored` compiles that C source
// instead, so a Linux build needs no system packages. This was not a
// theoretical concern — without it the build fails on a clean machine with
// "The pkg-config command could not be found", which is the state CI is in.
//
// On Linux that backend reaches the session bus, so a headless machine with no
// Secret Service daemon gets an error from `get` and takes the key-file path
// instead.

use clio_types::{AmError, ErrorCode};
use std::collections::HashMap;

/// Service the keys are filed under in the OS secret store.
pub(crate) const SERVICE: &str = "clio";

/// Account the encryption key is filed under in the OS secret store.
pub(crate) const ACCOUNT: &str = "encryption-key";

/// Account the sync key is filed under, distinct from the encryption key's.
///
/// The two keys have different lifetimes and different blast radii, so neither
/// may read the other by accident.
pub(crate) const SYNC_ACCOUNT: &str = "sync-key";

/// Operator-facing noun for the encryption key in store messages.
pub(crate) const ENCRYPTION_KEY: &str = "the Clio encryption key";

/// Operator-facing noun for the sync key in store messages.
pub(crate) const SYNC_KEY: &str = "the Clio sync key";

/// One store that can hand back the Clio encryption key.
///
/// A trait rather than a direct `keyring` call so the precedence rules can be
/// tested without touching the machine's real Keychain or Secret Service. An
/// implementation reports a locked, denied, or absent backend as an error;
/// `Ok(None)` means "reachable, and there is no entry".
pub trait SecretStore {
    /// Read the stored key bytes, or `None` when the store holds no entry.
    ///
    /// # Errors
    /// The store is unreachable, locked, or refused the read.
    fn get(&self) -> Result<Option<Vec<u8>>, AmError>;

    /// Store `bytes` as the key, replacing any existing entry.
    ///
    /// # Errors
    /// The store refused the write.
    fn set(&self, bytes: &[u8]) -> Result<(), AmError>;

    /// Remove the entry, succeeding when there is nothing to remove.
    ///
    /// # Errors
    /// The store refused the delete.
    fn delete(&self) -> Result<(), AmError>;

    /// Read the non-secret named attributes of the entry, if the store has any.
    ///
    /// A store that does not support named attributes returns an empty map
    /// rather than failing: "this store does not retain that" is the answer.
    ///
    /// # Errors
    /// The store is unreachable, locked, or refused the read.
    fn attributes(&self) -> Result<HashMap<String, String>, AmError> {
        Ok(HashMap::new())
    }

    /// Set one non-secret named attribute on the entry.
    ///
    /// A store that ignores unsupported attribute names returns `Ok(())` without
    /// storing anything, because that is what the `keyring` layer does. The
    /// caller reads the attribute back before believing it was kept.
    ///
    /// # Errors
    /// The store is unreachable, locked, or refused the write.
    fn set_attribute(&self, _name: &str, _value: &str) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "this key store cannot record attributes",
        ))
    }
}

/// Builds the `keyring` entry every operation goes through.
///
/// The account name is a parameter rather than a captured constant so one
/// wrapper serves both key roles and a test can prove which account it reached.
pub(crate) type EntryFactory = Box<dyn Fn(&str) -> keyring::Result<keyring::Entry> + Send + Sync>;

/// A store that is not there at all: the key file is the only key.
///
/// This exists because the lifecycle rules always ask the keystore one question
/// — is there also a key here? — and on macOS that question is a Keychain
/// read, which the operating system may answer with a password prompt. For an
/// operator who knows the key file is the only key — a headless box, a
/// container, CI, or a test run — that prompt is pure cost, repeated by every
/// process that starts.
///
/// Reporting "reachable and empty" and refusing writes composes with the
/// existing rules rather than adding a branch to them: a key file that exists
/// stays authoritative, and a missing one is created in the file because the
/// write is refused. Nothing outside this module has to know.
pub struct NoKeystore;

impl SecretStore for NoKeystore {
    fn get(&self) -> Result<Option<Vec<u8>>, AmError> {
        Ok(None)
    }

    fn set(&self, _bytes: &[u8]) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            "the key file is the only Clio key store, so there is no keystore to write to",
        ))
    }

    fn delete(&self) -> Result<(), AmError> {
        Ok(())
    }
}

/// The production [`SecretStore`]: one `keyring` entry per machine.
///
/// The entry is built through an injected factory rather than inline, which is
/// the second half of the test seam. [`Self::os`] supplies the real OS store; a
/// test supplies an entry over an in-memory credential and exercises this whole
/// impl, including the `keyring` error mapping, without any store being touched.
///
/// The account name is the only thing that separates the encryption key from the
/// sync key in the store, so it is carried explicitly and passed to the factory.
pub(crate) struct KeyringStore {
    account: &'static str,
    what: &'static str,
    build: EntryFactory,
}

impl KeyringStore {
    /// The store backed by the operating system's secret store.
    pub(crate) fn os(account: &'static str, what: &'static str) -> Self {
        Self {
            account,
            what,
            build: Box::new(os_entry),
        }
    }

    /// A store over an arbitrary `keyring` entry, so the wrapper above can be
    /// tested without an OS store behind it.
    #[cfg(test)]
    pub(crate) fn over<F>(account: &'static str, what: &'static str, build: F) -> Self
    where
        F: Fn(&str) -> keyring::Result<keyring::Entry> + Send + Sync + 'static,
    {
        Self {
            account,
            what,
            build: Box::new(build),
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self) -> Result<Option<Vec<u8>>, AmError> {
        match self.entry()?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            // "No entry" is the answer to the question, not a failure to read.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(store_error("read from", self.what, &err)),
        }
    }

    fn set(&self, bytes: &[u8]) -> Result<(), AmError> {
        // Raw bytes, not a base64 string: the store's byte API exists, so the
        // keystore and the key file hold the same 32 bytes and the length check
        // downstream is one byte count either way.
        self.entry()?
            .set_secret(bytes)
            .map_err(|err| store_error("write to", self.what, &err))
    }

    fn delete(&self) -> Result<(), AmError> {
        match self.entry()?.delete_credential() {
            // Deleting an absent entry is the state the caller asked for.
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(store_error("delete from", self.what, &err)),
        }
    }

    fn attributes(&self) -> Result<HashMap<String, String>, AmError> {
        match self.entry()?.get_attributes() {
            Ok(attributes) => Ok(attributes),
            // A store that keeps the entry answers even when it keeps no
            // attributes; "no entry" here means the caller asked too early.
            Err(keyring::Error::NoEntry) => Ok(HashMap::new()),
            Err(err) => Err(store_error("read the attributes of", self.what, &err)),
        }
    }

    fn set_attribute(&self, name: &str, value: &str) -> Result<(), AmError> {
        let mut attributes = HashMap::new();
        attributes.insert(name, value);
        self.entry()?
            .update_attributes(&attributes)
            .map_err(|err| store_error("write an attribute of", self.what, &err))
    }
}

impl KeyringStore {
    /// The entry this store operates on, with a failure to build it reported the
    /// same way as any other keystore failure.
    fn entry(&self) -> Result<keyring::Entry, AmError> {
        (self.build)(self.account).map_err(|err| store_error("open", self.what, &err))
    }
}

/// The one entry this build uses in production: the OS secret store.
fn os_entry(account: &str) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, account)
}

/// Operator-facing name of the OS secret store on this platform.
///
/// Named from the backend `keyring` actually selects, so the message matches
/// what the operator has to go and look at.
#[must_use]
pub(crate) fn display_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else if cfg!(target_os = "linux") {
        "Linux Secret Service"
    } else {
        "the OS secret store"
    }
}

/// A keystore failure as a Clio error, with nothing from the store in the text.
///
/// The `keyring` error is classified rather than rendered. Two of its variants
/// carry store contents — `BadEncoding` holds the raw bytes and `Ambiguous`
/// formats whole credentials, secret included — so printing it would put key
/// material into a message that ends up in logs and on a terminal.
pub(crate) fn store_error(action: &str, what: &str, err: &keyring::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!(
            "cannot {action} {what} in {}: {}",
            display_name(),
            classify(err)
        ),
    )
}

/// Secret-free summary of a `keyring` failure.
///
/// The wildcard arm is required because the upstream enum is
/// `#[non_exhaustive]`, and it is the one arm no test can reach: an unknown
/// variant cannot be constructed from outside the crate. It is also the arm that
/// keeps an unrecognised failure from falling through to a rendered error, so it
/// is the arm that must never grow a `{err}`.
pub(crate) fn classify(err: &keyring::Error) -> &'static str {
    match err {
        keyring::Error::NoEntry => "there is no entry",
        keyring::Error::NoStorageAccess(_) => "the store is locked or refused access",
        keyring::Error::PlatformFailure(_) => "the platform store failed",
        keyring::Error::BadEncoding(_) => "the stored value is not readable",
        keyring::Error::TooLong(_, _) => "an entry attribute is too long",
        keyring::Error::Invalid(_, _) => "an entry attribute is invalid",
        _ => "the store reported a failure this build does not classify",
    }
}

#[cfg(test)]
#[path = "secret_key_store_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "secret_key_store_attribute_tests.rs"]
mod attribute_tests;
