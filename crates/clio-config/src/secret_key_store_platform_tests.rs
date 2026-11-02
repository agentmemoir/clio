// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the platform store selection and
//! installation: that the store this build installs connects or says why, that
//! an install reaches the process default, and that a store which could not be
//! installed is reported as a Clio error naming the platform.
//!
//! ## Owns
//! - The test that connects this target's real store and accepts "no daemon"
//! - The test that opens the production entry over the installed store
//! - The test that installs a store with no backend through the install seam
//! - The refusal arm, driven by seeding the install outcome the production entry reads
//! - The rendering of a connect failure
//!
//! ## Does not own
//! - The entry lifecycle over an in-memory credential (see
//!   `secret_key_store_tests`)
//! - The platform store crates themselves
//!
//! ## Boundary
//!
//! Two tests here install a real OS store, and none of them reads a secret: a
//! store is connected and an entry is built over it, never queried, so no test
//! can raise a Keychain password prompt. The install seam is driven with a store
//! that has no backend behind it, which is how the success lines are reached on
//! a host whose own store does not answer.
//!
//! Keep this module focused on the installation of the platform store.

use super::*;
use crate::secret_key_store::ACCOUNT;
use crate::secret_key_store_test_support::FakeStore;
use clio_types::ErrorCode;
use keyring_core::api::CredentialStoreApi;

// ------------------------------------------------- the store this build installs

#[cfg(target_os = "macos")]
#[test]
fn the_store_this_target_installs_opens_on_the_user_keychain() {
    // `Store::new()` for the Keychain records the domain and nothing else: it
    // opens no connection and reads no item, so this touches nothing an operator
    // would be asked to unlock or approve.
    install_platform_store().expect("the user Keychain needs no session to open a store");
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn the_store_this_target_installs_connects_or_says_why() {
    // This store opens its session-bus connection inside `Store::new()`, so on a
    // runner with no daemon this is the failing path and its reason is the text
    // an operator gets. Either outcome is correct; what must not happen is a
    // bare `NoDefaultStore` with no cause attached.
    match install_platform_store() {
        Ok(()) => assert!(
            keyring_core::get_default_store().is_some(),
            "an install that reports success leaves the store installed"
        ),
        Err(reason) => {
            assert!(!reason.trim().is_empty(), "a failed connect still says why");
            assert!(
                classified(&reason),
                "the reason opens with one of the store's own summaries, not a rendered error: {reason}"
            );
        }
    }
}

// ------------------------------------------------ the entry over the installed store

#[cfg(target_os = "macos")]
#[test]
fn the_production_entry_opens_over_the_store_this_build_installs() {
    // The whole production path up to, but not including, the read that would ask
    // the operator for a password: the store is installed once for the process,
    // and the entry is built over whichever store that left.
    assert!(
        os_entry(ACCOUNT).is_ok(),
        "the Keychain store installs and opens an entry"
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn the_production_entry_opens_or_reports_the_install_reason() {
    if let Err(err) = os_entry(ACCOUNT) {
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(
            err.message.starts_with("cannot open an entry in "),
            "{}",
            err.message
        );
        assert!(err.message.contains(display_name()), "{}", err.message);
    }
}

// --------------------------------------------------------------- the install seam

#[test]
fn installing_a_store_reaches_the_process_default() {
    // The success lines of the install are unreachable from a host whose own
    // store does not answer, so they are driven here with a store that has no
    // backend: installing publishes the store and reads no credential from it.
    //
    // The store is left installed on purpose. The only reader of the process
    // default in this crate's tests is the production entry, which accepts
    // whatever is installed; unsetting it here would race that read inside
    // `cargo test`'s one process and fail a test for a reason its failure would
    // not show.
    let fake = Arc::new(FakeStore);
    install(|| Ok(fake)).expect("a store with no backend installs without a reason");
    assert!(
        keyring_core::get_default_store().is_some(),
        "an install that reports success leaves a store installed"
    );
}

#[test]
fn fake_store_builds_an_isolated_in_memory_entry() {
    let store = FakeStore;
    assert_ne!(store.vendor(), "");
    assert_ne!(store.id(), "");
    assert!(store.as_any().is::<FakeStore>());

    let first = store.build("service", "user", None).expect("first entry");
    let second = store.build("service", "user", None).expect("second entry");
    first.set_secret(b"test secret").expect("write");
    assert_eq!(first.get_secret().expect("read"), b"test secret");
    assert!(matches!(
        second.get_secret(),
        Err(keyring_core::Error::NoEntry)
    ));
}

#[test]
fn an_entry_over_a_failed_install_reports_the_reason_and_names_the_platform() {
    // The refusal arm. The install outcome is seeded for this thread only, so the
    // production entry — not a stand-in for it — is driven to its failure arm on a
    // host whose own store answers. No store was installed, so no entry is
    // attempted and the operator is told which store failed and why, rather than
    // being handed the `NoDefaultStore` that `Entry::new` would answer with.
    seed_install(Err("no session bus".to_owned()));

    let err = os_entry(ACCOUNT).expect_err("a store that was not installed opens no entry");

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message.starts_with("cannot open an entry in "),
        "{}",
        err.message
    );
    assert!(err.message.contains(display_name()), "{}", err.message);
    assert!(
        err.message.contains("no session bus"),
        "the cause an operator has to fix is carried through: {}",
        err.message
    );
}

#[test]
fn a_connect_failure_is_summarised_before_the_platform_error() {
    // `Store::new()` runs before any entry exists, so the platform error it
    // carries cannot hold a stored value; it is still prefixed with the
    // classified summary, so the message opens with the store's own verdict.
    let err =
        keyring_core::Error::PlatformFailure(Box::new(std::io::Error::other("no session bus")));

    let reason = install_error(&err);

    assert!(
        reason.starts_with("the platform store failed: "),
        "the summary comes first: {reason}"
    );
    assert!(
        reason.contains("no session bus"),
        "the cause follows it: {reason}"
    );
}

/// Whether `reason` opens with one of the summaries `classify` gives.
///
/// All eleven variants the upstream type names are asked for their summary, so
/// this holds for whatever failure a store's `new()` reports without naming one
/// here. `Ambiguous` is built over an empty vector because `classify` reads no
/// entry out of it, only its fixed wording.
#[cfg(all(unix, not(target_os = "macos")))]
fn classified(reason: &str) -> bool {
    let shapes = [
        keyring_core::Error::NoEntry,
        keyring_core::Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))),
        keyring_core::Error::PlatformFailure(Box::new(std::io::Error::other("bus down"))),
        keyring_core::Error::BadEncoding(vec![0xAB; 4]),
        keyring_core::Error::BadDataFormat(
            vec![0xAB; 4],
            Box::new(std::io::Error::other("not a sealed blob")),
        ),
        keyring_core::Error::BadStoreFormat("header".to_owned()),
        keyring_core::Error::TooLong("service".to_owned(), 900),
        keyring_core::Error::Invalid("service".to_owned(), "empty".to_owned()),
        keyring_core::Error::Ambiguous(vec![]),
        keyring_core::Error::NoDefaultStore,
        keyring_core::Error::NotSupportedByStore("attributes".to_owned()),
    ];

    shapes
        .iter()
        .any(|shape| reason.starts_with(crate::secret_key_store::classify(shape)))
}
