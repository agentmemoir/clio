// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for which secret store this build installs into
//! `keyring_core`, for installing it once per process — lazily, on the first
//! `Entry::new`, not at process start — and for opening the one production entry
//! over it.
//!
//! ## Owns
//! - Choosing the store: the Keychain, the Credential Manager, the Secret
//!   Service, or a refusal for a platform this build has no store for
//! - `STORE_INIT`, the once-per-process outcome of that install
//! - [`install`], the install step on its own, so a test can drive it with a
//!   store that is not an OS store
//! - `seed_install`, the test-only seam that supplies the install outcome
//! - [`os_entry`], the production entry, and the message a failed install gives
//! - Rendering a connect failure as operator-facing text
//!
//! ## Does not own
//! - The [`SecretStore`](super::SecretStore) seam and the `keyring_core` wrapper
//!   over it (see `secret_key_store`)
//! - The operator-facing name of the store (see `super::display_name`)
//! - The secret-free summary of a `keyring_core` failure (see `super::classify`)
//!
//! ## Boundary
//!
//! Changes to which crate provides the OS store, or to when it is installed,
//! should normally require changes here. Changes to what an entry is called, or
//! to how a caller reaches a key, should not.
//!
//! Keep this module focused on the platform store and its installation.
//!
//! # Why there is no `keyring` facade
//!
//! `keyring-core` owns the entry type, the credential trait, and the error. The
//! store behind it is chosen per target here — the Keychain on macOS, the
//! Credential Manager on Windows, the Secret Service on Unix — and installed
//! once by [`install_store`]. There is deliberately no `keyring` facade in
//! between: its `v1` feature selects a `zbus` Secret Service store, and RustSec
//! reports `zbus`, `zbus_names`, and `zvariant` unmaintained with no patched
//! version, which `deny.toml` refuses.
//!
//! # Why the install is cached, and why its failure is reported
//!
//! The installed store is cached in `STORE_INIT` rather than re-installed on
//! every entry: `keyring-core` keeps one default store per process, and a
//! failure is the same failure every time it is asked. Each store connects to
//! its backend inside `Store::new()`, so a headless machine with no Secret
//! Service daemon fails there. That failure is reported rather than swallowed:
//! no store is installed, and the keystore path answers with the platform error
//! instead of `get` failing later. The keystore path still refuses — it never
//! redirects the key into a file the operator did not choose — and a machine
//! that never chose the keystore reads the key file instead, because an absent
//! marker means the file store.
//!
//! On Unix the Secret Service is reached over D-Bus through the C libdbus this
//! crate compiles in (`vendored`), and the session is sealed with RustCrypto
//! AES/CBC (`crypto-rust`). So the Linux build links no OpenSSL and probes no
//! `pkg-config` for either library; it does need a C compiler for the bundled
//! libdbus, which is what master already builds today.

use clio_types::{AmError, ErrorCode};
use keyring_core::api::CredentialStoreApi;
#[cfg(test)]
use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use super::{SERVICE, display_name, store_error};

/// The result of installing this platform's store, cached for the process.
///
/// `keyring-core` keeps the default store in one global, so installing is a
/// once-per-process act and a second install would only replace it. Caching the
/// outcome means a machine that cannot reach its store reports the same reason
/// every time rather than retrying a connect that will keep failing.
static STORE_INIT: OnceLock<Result<(), String>> = OnceLock::new();

// An install outcome seeded for this thread, so `os_entry` can be driven to its
// failure arm on a host whose own store answers (see `seed_install`).
//
// `STORE_INIT` is process-wide and cannot be reset, so a seed there would decide
// what every later test in the binary read. A thread-local seed is one test's own
// state — each test runs on its own thread — and it is taken by the one call that
// reads it, so no test can see another's. Test-only: no build of this crate ships
// a way to skip the platform store.
#[cfg(test)]
thread_local! {
    static SEEDED_INSTALL: RefCell<Option<Result<(), String>>> = const { RefCell::new(None) };
}

/// Seed the install outcome this thread's next [`install_store`] call reports.
#[cfg(test)]
pub(super) fn seed_install(outcome: Result<(), String>) {
    SEEDED_INSTALL.with(|seeded| *seeded.borrow_mut() = Some(outcome));
}

/// The outcome seeded for this thread, taken so it is reported once.
#[cfg(test)]
fn seeded_install() -> Option<Result<(), String>> {
    SEEDED_INSTALL.with(RefCell::take)
}

/// The one entry this build uses in production: the OS secret store.
///
/// The platform store is installed before the entry is built, because
/// `keyring_core::Entry::new` reaches whichever store was installed and answers
/// `NoDefaultStore` when none was. Installation happens here rather than in the
/// wrapper's entry seam so a test entry never touches the machine's real store.
///
/// A store that was not installed is reported here rather than left to
/// `Entry::new`, whose `NoDefaultStore` says nothing about the cause.
pub(super) fn os_entry(account: &str) -> Result<keyring_core::Entry, AmError> {
    install_store().map_err(|reason| {
        AmError::new(
            ErrorCode::Internal,
            format!("cannot open an entry in {}: {reason}", display_name()),
        )
    })?;
    keyring_core::Entry::new(SERVICE, account).map_err(|err| store_error("open", "an entry", &err))
}

/// Install the platform store, once per process, reporting the cached outcome.
fn install_store() -> Result<(), String> {
    #[cfg(test)]
    if let Some(seeded) = seeded_install() {
        return seeded;
    }
    STORE_INIT.get_or_init(install_platform_store).clone()
}

/// Connect the store `connect` produces and make it this process's default.
///
/// The connect step is a parameter rather than a call inside, which is the seam:
/// a test connects a store with no backend behind it and reaches the success
/// lines that a host whose own OS store does not answer can never reach. The
/// failure it reports is the connect failure, rendered by [`install_error`].
fn install<C>(connect: impl FnOnce() -> keyring_core::Result<Arc<C>>) -> Result<(), String>
where
    C: CredentialStoreApi + Send + Sync + 'static,
{
    let store = connect().map_err(|err| install_error(&err))?;
    keyring_core::set_default_store(store);
    Ok(())
}

/// macOS: the Keychain, at the user domain. `default-features = false` plus the
/// `keychain` feature leaves this the only backend the crate compiles in.
///
/// `Store::new()` records the domain and opens no connection, so a store exists
/// on every Mac; the connect still goes through [`install`] so a failure has one
/// shape whatever the platform.
#[cfg(target_os = "macos")]
fn install_platform_store() -> Result<(), String> {
    install(apple_native_keyring_store::keychain::Store::new)
}

/// Windows: the Credential Manager, through the Win32 credential API.
#[cfg(target_os = "windows")]
fn install_platform_store() -> Result<(), String> {
    install(windows_native_keyring_store::Store::new)
}

/// Unix: the Secret Service, over the vendored libdbus.
///
/// This store opens its session-bus connection inside `Store::new()`, so a
/// machine with no Secret Service daemon fails there rather than on the first
/// read, and the reason is the message the operator gets.
#[cfg(all(unix, not(target_os = "macos")))]
fn install_platform_store() -> Result<(), String> {
    install(dbus_secret_service_keyring_store::Store::new)
}

/// Refuse a platform this build has no store for, in words rather than with a
/// `NoDefaultStore` that says nothing about the cause.
#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(unix, not(target_os = "macos"))
)))]
fn install_platform_store() -> Result<(), String> {
    Err(format!(
        "this platform ({}) has no Clio key store: this build has the macOS Keychain, the \
         Windows Credential Manager, and the Unix Secret Service, and none of them applies here",
        std::env::consts::OS
    ))
}

/// The reason a store could not be installed, as operator-facing text.
///
/// This is the one place a platform error is rendered instead of classified.
/// `Store::new()` runs before any entry exists, so no stored value is in scope
/// and none of the `keyring_core` variants that carry one can occur; what the
/// text holds is the connect failure — no session bus, a refused socket, no
/// login keychain — which is what the operator has to fix. [`super::classify`]
/// still prefixes it, so the message opens with the store's own summary.
fn install_error(err: &keyring_core::Error) -> String {
    format!("{}: {err}", super::classify(err))
}

#[cfg(test)]
#[path = "secret_key_store_platform_tests.rs"]
mod tests;

/// The one test that needs a real Secret Service daemon, so it is off unless the `linux-secret-service-test` feature asks for it (see the module doc for why it cannot run everywhere).
#[cfg(all(test, target_os = "linux", feature = "linux-secret-service-test"))]
#[path = "secret_key_store_live_tests.rs"]
mod live_tests;
