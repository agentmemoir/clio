// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared keystore test scaffolding: an
//! in-memory `keyring` credential with configurable failures and attribute
//! support, and the wrappers that drive the production [`KeyringStore`] through
//! it.
//!
//! ## Owns
//! - [`FakeCredential`], an in-memory credential with attribute support
//! - [`PlainCredential`], a credential whose store keeps no attributes
//! - The factory wiring that keeps every test off the real OS store
//!
//! ## Does not own
//! - Any test (see `secret_key_store_tests` and
//!   `secret_key_store_attribute_tests`)
//! - The production store (see `secret_key_store`)
//!
//! ## Boundary
//!
//! There is no constructor here that produces a production store. A test that
//! drives [`KeyringStore`] does it through one of these credentials, so no test
//! can reach the machine's real Keychain or Secret Service.
//!
//! Keep this module focused on test scaffolding, with no tests of its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use clio_types::AmError;

use crate::secret_key_store::{ACCOUNT, ENCRYPTION_KEY, KeyringStore};

/// How a [`FakeCredential`] should fail, as a shape rather than a value.
///
/// `keyring::Error` is neither `Clone` nor `Copy`, so the fake records which
/// failure to raise and builds a fresh one per call.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Failure {
    /// Behaves normally.
    #[default]
    None,
    /// The store is locked or refuses access.
    Locked,
    /// The platform store itself failed.
    Platform,
}

impl Failure {
    fn to_error(self) -> keyring::Error {
        match self {
            Self::None => keyring::Error::NoEntry,
            Self::Locked => keyring::Error::NoStorageAccess(Box::new(std::io::Error::other(
                "the store is locked",
            ))),
            Self::Platform => keyring::Error::PlatformFailure(Box::new(std::io::Error::other(
                "the session bus is down",
            ))),
        }
    }
}

/// An in-memory `keyring` credential, so the wrapper can be tested without an OS
/// store. Cloned handles share one secret, which is how a test watches what the
/// store wrote.
///
/// This fake keeps named attributes (as the Linux Secret Service does). The
/// separate [`PlainCredential`] has no attribute support, which is how the
/// macOS and Windows no-op path is exercised.
#[derive(Clone, Default)]
pub(crate) struct FakeCredential {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    secret: Option<Vec<u8>>,
    read: Failure,
    write: Failure,
    delete: Failure,
    attributes: HashMap<String, String>,
    attributes_failure: Option<Failure>,
}

impl FakeCredential {
    /// A reachable, empty store.
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    /// Reachable, holding `bytes`.
    pub(crate) fn holding(bytes: &[u8]) -> Self {
        FakeCredential::configure(|state| state.secret = Some(bytes.to_vec()))
    }

    /// Reads fail as a locked store's do.
    pub(crate) fn locked_on_read() -> Self {
        FakeCredential::configure(|state| state.read = Failure::Locked)
    }

    /// Reads succeed and writes fail, as a read-only store's do.
    pub(crate) fn failing_writes() -> Self {
        FakeCredential::configure(|state| state.write = Failure::Platform)
    }

    /// Deletes fail, as a store refusing removal does.
    pub(crate) fn failing_deletes() -> Self {
        FakeCredential::configure(|state| state.delete = Failure::Locked)
    }

    /// Attribute reads and writes fail, as a locked store's do.
    pub(crate) fn failing_attributes() -> Self {
        FakeCredential::configure(|state| state.attributes_failure = Some(Failure::Locked))
    }

    /// Reachable, holding `bytes`, whose attribute reads and writes fail.
    ///
    /// The secret is readable because the creation time is only worth asking
    /// about once there is a key to ask about.
    pub(crate) fn holding_with_failing_attributes(bytes: &[u8]) -> Self {
        FakeCredential::configure(|state| {
            state.secret = Some(bytes.to_vec());
            state.attributes_failure = Some(Failure::Locked);
        })
    }

    /// Start refusing attribute reads and writes on an already-built credential.
    ///
    /// Cloned handles share one state, so a test can store a key first and only
    /// then make the store refuse, which is the order the sync key does it in.
    pub(crate) fn refuse_attributes_now(&self) {
        self.lock().attributes_failure = Some(Failure::Locked);
    }

    fn configure(build: impl FnOnce(&mut FakeState)) -> Self {
        let credential = Self::default();
        build(&mut credential.lock());
        credential
    }

    pub(crate) fn secret(&self) -> Option<Vec<u8>> {
        self.lock().secret.clone()
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().expect("fake credential mutex")
    }
}

impl keyring::credential::CredentialApi for FakeCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        let mut state = self.lock();
        if state.write != Failure::None {
            return Err(state.write.to_error());
        }
        state.secret = Some(secret.to_vec());
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        let state = self.lock();
        if state.read != Failure::None {
            return Err(state.read.to_error());
        }
        state.secret.clone().ok_or(keyring::Error::NoEntry)
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        let mut state = self.lock();
        if state.delete != Failure::None {
            return Err(state.delete.to_error());
        }
        state.secret = None;
        Ok(())
    }

    fn get_attributes(&self) -> keyring::Result<HashMap<String, String>> {
        let state = self.lock();
        if let Some(failure) = state.attributes_failure {
            return Err(failure.to_error());
        }
        if state.read != Failure::None {
            return Err(state.read.to_error());
        }
        if state.secret.is_none() {
            return Err(keyring::Error::NoEntry);
        }
        Ok(state.attributes.clone())
    }

    fn update_attributes(&self, attributes: &HashMap<&str, &str>) -> keyring::Result<()> {
        let mut state = self.lock();
        if let Some(failure) = state.attributes_failure {
            return Err(failure.to_error());
        }
        if state.read != Failure::None {
            return Err(state.read.to_error());
        }
        if state.secret.is_none() {
            return Err(keyring::Error::NoEntry);
        }
        for (name, value) in attributes {
            state
                .attributes
                .insert((*name).to_owned(), (*value).to_owned());
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A credential with no attribute support at all.
///
/// Its `get_attributes` / `update_attributes` are `keyring`'s defaults: the
/// write reports success while storing nothing, exactly as the macOS and
/// Windows backends do. That is the behaviour the sync key's creation time has
/// to survive.
#[derive(Clone)]
pub(crate) struct PlainCredential(FakeCredential);

impl PlainCredential {
    /// A credential whose store keeps no named attributes.
    pub(crate) fn without_attributes() -> Self {
        Self(FakeCredential::empty())
    }

    /// The same, over a credential that already holds a secret.
    pub(crate) fn without_attributes_over(credential: &FakeCredential) -> Self {
        Self(credential.clone())
    }
}

impl keyring::credential::CredentialApi for PlainCredential {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        self.0.set_secret(secret)
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.0.get_secret()
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        self.0.delete_credential()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The production wrapper, wired to `credential` exactly as it is wired to the OS
/// store in production: through keyring's own `Entry`.
pub(crate) fn store_over(credential: &FakeCredential) -> KeyringStore {
    let held = credential.clone();
    KeyringStore::over(ACCOUNT, ENCRYPTION_KEY, move |account| {
        assert_eq!(
            account, ACCOUNT,
            "the data key opens the data key's account"
        );
        Ok(keyring::Entry::new_with_credential(Box::new(held.clone())))
    })
}

/// The production wrapper over any in-memory credential, cloned per entry.
pub(crate) fn store_over_credential<C>(
    account: &'static str,
    what: &'static str,
    credential: C,
) -> KeyringStore
where
    C: keyring::credential::CredentialApi + Clone + Send + Sync + 'static,
{
    KeyringStore::over(account, what, move |_| {
        Ok(keyring::Entry::new_with_credential(Box::new(
            credential.clone(),
        )))
    })
}

/// Unwrap the refusal from a result that was expected to fail.
pub(crate) fn err_of(result: Result<impl std::fmt::Debug, AmError>) -> AmError {
    result.expect_err("expected a refusal")
}
