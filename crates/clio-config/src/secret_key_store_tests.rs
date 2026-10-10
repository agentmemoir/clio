// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the `keyring` wrapper: that a
//! missing entry reads as "no key" rather than a failure, that a locked or
//! broken store becomes a Clio error naming the store, and that nothing the
//! store holds reaches a message.
//!
//! ## Owns
//! - The [`KeyringStore`] read, write, and delete tests
//! - The [`NoKeystore`] answers, which are the other store this module defines
//! - The message tests, including the two upstream variants that carry bytes
//!
//! ## Does not own
//! - The injected credential and factory (see `secret_key_store_test_support`)
//! - The attribute methods (see `secret_key_store_attribute_tests`)
//! - Which store wins (see `secret_key_tests`)
//!
//! ## Boundary
//!
//! These tests drive the production [`KeyringStore`] through keyring's own
//! `Entry`, over an in-memory credential, so no OS store is reached. The one
//! function with no test is `os_entry`, and it is the only code that names the
//! real Keychain.
//!
//! Keep this module focused on the wrapper's entry lifecycle.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::secret_key_store_test_support::{FakeCredential, err_of, store_over};
use clio_types::ErrorCode;

// ------------------------------------------------------- the store that is not there

#[test]
fn the_store_that_is_not_there_is_reachable_and_holds_nothing() {
    // This is the store a headless box, a container, or a test run gets, and the
    // whole point of it is that asking costs nothing: no Keychain read, so no
    // password prompt on macOS. That only holds if it answers, and answers "no
    // entry" rather than failing, because a key file that already exists has to
    // stay authoritative and a caller cannot tell an absent keystore from a broken
    // one by an error.
    let store = NoKeystore;

    assert_eq!(
        store.get().expect("reachable and empty"),
        None,
        "an absent keystore holds no key and says so"
    );
    store
        .delete()
        .expect("deleting from a store that holds nothing is what was asked for");
}

#[test]
fn the_store_that_is_not_there_refuses_a_write_and_names_the_file_as_the_only_key() {
    // The refusal is the point: a caller that cannot write the keystore creates the
    // key in the file instead, and that fallback is invisible to everything outside
    // this module. The message has to say which store is the only one, because the
    // operator is being told a write failed and needs to know it will land in a file
    // instead.
    let store = NoKeystore;

    let err = err_of(store.set(&[7_u8; KEY_LEN]));

    assert!(
        err.message
            .contains("the key file is the only Clio key store"),
        "{}",
        err.message
    );
}

// ------------------------------------------------------------------ reading

#[test]
fn an_empty_store_reads_as_no_key_rather_than_a_failure() {
    let credential = FakeCredential::empty();
    let store = store_over(&credential);

    let found = store.get().expect("reachable and empty");

    assert_eq!(found, None, "no entry is an answer, not an error");
}

#[test]
fn a_stored_key_reads_back_byte_for_byte() {
    let wanted = rand::random::<[u8; KEY_LEN]>();
    let credential = FakeCredential::holding(&wanted);
    let store = store_over(&credential);

    let found = store.get().expect("read").expect("an entry");

    assert_eq!(found, wanted, "the store holds raw bytes, not an encoding");
}

#[test]
fn a_locked_store_reads_as_an_error_naming_the_store() {
    let store = store_over(&FakeCredential::locked_on_read());

    let err = err_of(store.get());

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains(display_name()), "{}", err.message);
    assert!(
        err.message.contains("locked or refused access"),
        "{}",
        err.message
    );
}

// ------------------------------------------------------------------ writing

#[test]
fn a_written_key_lands_in_the_store_unchanged() {
    let wanted = rand::random::<[u8; KEY_LEN]>();
    let credential = FakeCredential::empty();
    let store = store_over(&credential);

    store.set(&wanted).expect("write");

    assert_eq!(credential.secret().expect("stored"), wanted.to_vec());
    assert_eq!(
        store.get().expect("read").expect("an entry"),
        wanted.to_vec()
    );
}

#[test]
fn writing_replaces_the_previous_key_rather_than_adding_one() {
    let first = rand::random::<[u8; KEY_LEN]>();
    let second = rand::random::<[u8; KEY_LEN]>();
    let credential = FakeCredential::empty();
    let store = store_over(&credential);

    store.set(&first).expect("first");
    store.set(&second).expect("second");

    assert_eq!(credential.secret().expect("stored"), second.to_vec());
    assert_eq!(
        store.get().expect("read").expect("an entry"),
        second,
        "one entry holds one key"
    );
}

#[test]
fn a_store_that_refuses_writes_says_so() {
    let store = store_over(&FakeCredential::failing_writes());

    let err = err_of(store.set(&[0_u8; KEY_LEN]));

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message
            .contains("cannot write to the Clio encryption key"),
        "{}",
        err.message
    );
    assert!(err.message.contains(display_name()), "{}", err.message);
}

// ----------------------------------------------------------------- deleting

#[test]
fn deleting_removes_the_entry_and_is_repeatable() {
    let credential = FakeCredential::holding(&rand::random::<[u8; KEY_LEN]>());
    let store = store_over(&credential);

    store.delete().expect("delete");
    assert_eq!(credential.secret(), None);
    store.delete().expect("deleting nothing is still success");
    assert_eq!(store.get().expect("read"), None);
}

#[test]
fn a_store_that_refuses_deletion_says_so() {
    let store = store_over(&FakeCredential::failing_deletes());

    let err = err_of(store.delete());

    assert!(
        err.message
            .contains("cannot delete from the Clio encryption key"),
        "{}",
        err.message
    );
}

// ---------------------------------------------------------------- messages

#[test]
fn no_keystore_failure_message_carries_the_stored_bytes() {
    let wanted = rand::random::<[u8; KEY_LEN]>();
    // `BadEncoding` holds the stored bytes and `Ambiguous` formats whole
    // credentials with `Debug`. Those are the two variants that would leak, so
    // they are the two that must be rendered here.
    let leaks = [
        keyring::Error::BadEncoding(wanted.to_vec()),
        keyring::Error::Ambiguous(vec![Box::new(SecretCredential(wanted))]),
    ];

    for err in &leaks {
        let message = store_error("read from", ENCRYPTION_KEY, err).message;

        assert!(message.contains(display_name()), "{message}");
        assert!(
            !message.contains(&hex_of(&wanted)),
            "a rendered keystore error leaked the stored bytes: {message}"
        );
    }
}

#[test]
fn the_production_store_can_be_built_without_reaching_the_keychain() {
    // `os` only records which entry to use; it does not open one, so this proves
    // the production wiring is intact without a single call to the OS store. The
    // entry itself is deliberately never built: on Linux that reaches the session
    // bus, which would hang on a CI runner that has none.
    let store = KeyringStore::os(ACCOUNT, ENCRYPTION_KEY);

    drop(store);
}

#[test]
fn every_failure_gets_its_own_wording() {
    let shapes = [
        keyring::Error::NoEntry,
        keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))),
        keyring::Error::PlatformFailure(Box::new(std::io::Error::other("bus down"))),
        keyring::Error::BadEncoding(vec![0xAB; 4]),
        keyring::Error::TooLong("service".to_owned(), 900),
        keyring::Error::Invalid("service".to_owned(), "empty".to_owned()),
    ];

    let mut summaries: Vec<&str> = shapes.iter().map(classify).collect();
    let count = summaries.len();
    summaries.sort_unstable();
    summaries.dedup();

    assert_eq!(
        summaries.len(),
        count,
        "each failure reads differently, so none is mistaken for another"
    );
    assert!(summaries.contains(&"there is no entry"));
}

/// Lower-case hex, the encoding a message would most plausibly leak a key in.
fn hex_of(key: &[u8; KEY_LEN]) -> String {
    use std::fmt::Write as _;
    key.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// A platform credential whose `Debug` prints its secret, the way a real
/// Keychain item does. `Ambiguous` formats its credentials with `Debug`, so
/// rendering that error is how key material would escape.
struct SecretCredential([u8; KEY_LEN]);

impl std::fmt::Debug for SecretCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret({})", String::from_utf8_lossy(&self.0))
    }
}

impl keyring::credential::CredentialApi for SecretCredential {
    fn set_secret(&self, _: &[u8]) -> keyring::Result<()> {
        Ok(())
    }

    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        Ok(self.0.to_vec())
    }

    fn delete_credential(&self) -> keyring::Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
