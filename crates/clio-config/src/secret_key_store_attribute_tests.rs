// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the named attributes of the
//! `keyring` wrapper and the account separation between the two key roles.
//!
//! ## Owns
//! - The attribute read/write tests, including the store that ignores them
//! - The test that each key role opens its own account
//!
//! ## Does not own
//! - The entry lifecycle (see `secret_key_store_tests`)
//! - The injected credential (see `secret_key_store_test_support`)
//! - The sync key's own use of attributes (see `sync_key_tests`)
//!
//! ## Boundary
//!
//! These tests run over an in-memory credential, so no test here reaches a real
//! OS store. The plain credential is the macOS and Windows shape: the write
//! reports success and stores nothing.
//!
//! Keep this module focused on attributes and account names.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::secret_key_store_test_support::{FakeCredential, PlainCredential, err_of, store_over};
use std::sync::{Arc, Mutex};

#[test]
fn each_key_role_opens_its_own_account() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    let store = KeyringStore::over(SYNC_ACCOUNT, SYNC_KEY, move |account| {
        recorder.lock().expect("seen").push(account.to_owned());
        Ok(keyring::Entry::new_with_credential(Box::new(
            FakeCredential::empty(),
        )))
    });

    store.get().expect("read");

    assert_eq!(
        &*seen.lock().expect("seen"),
        &["sync-key".to_owned()],
        "the sync key never reads through the encryption key's account"
    );
}

#[test]
fn an_attribute_a_store_keeps_round_trips_through_the_entry() {
    let credential = FakeCredential::holding(&[7_u8; KEY_LEN]);
    let store = crate::secret_key_store_test_support::store_over_credential(
        ACCOUNT,
        ENCRYPTION_KEY,
        credential,
    );

    store
        .set_attribute("created_at", "2026-10-01T00:00:00Z")
        .expect("the store keeps attributes");
    let attributes = store.attributes().expect("read back");

    assert_eq!(
        attributes.get("created_at").map(String::as_str),
        Some("2026-10-01T00:00:00Z")
    );
}

#[test]
fn an_attribute_a_store_ignores_reads_back_absent_rather_than_failing() {
    let credential = FakeCredential::holding(&[7_u8; KEY_LEN]);
    let store = crate::secret_key_store_test_support::store_over_credential(
        ACCOUNT,
        ENCRYPTION_KEY,
        PlainCredential::without_attributes_over(&credential),
    );

    // The no-op write reports success, which is why the caller must read back.
    store
        .set_attribute("created_at", "2026-10-01T00:00:00Z")
        .expect("a store without attribute support still reports success");
    let attributes = store.attributes().expect("read back");

    assert!(
        attributes.is_empty(),
        "an ignored attribute is absent, not an error and not a value"
    );
}

#[test]
fn a_store_that_cannot_record_an_attribute_says_so_rather_than_inventing_one() {
    // A store that does not keep named attributes still has to answer both
    // questions, because the caller asks before it believes anything. The read is
    // an empty map rather than an error: "this store does not retain that" is the
    // answer. The write is an error rather than a silent success, because the two
    // are not the same thing and the caller reads back to check which one happened.
    // `NoKeystore` is the production store that takes the trait's defaults here, so
    // this is the shape a run on a machine with no keystore actually meets.
    let store = NoKeystore;

    assert!(
        store
            .attributes()
            .expect("no attributes is an answer")
            .is_empty(),
        "a store that keeps no attributes reports none rather than failing"
    );
    let err = err_of(store.set_attribute("created_at", "2026-10-01T00:00:00Z"));
    assert!(
        err.message.contains("cannot record attributes"),
        "{}",
        err.message
    );
}

#[test]
fn a_locked_store_reports_an_attribute_read_error() {
    let store = store_over(&FakeCredential::locked_on_read());

    let err = err_of(store.attributes());

    assert!(
        err.message.contains("read the attributes of"),
        "{}",
        err.message
    );
    assert!(err.message.contains(display_name()), "{}", err.message);
}

#[test]
fn a_store_that_refuses_an_attribute_write_says_so() {
    let store = store_over(&FakeCredential::failing_attributes());

    let err = err_of(store.set_attribute("created_at", "2026-10-01T00:00:00Z"));

    assert!(
        err.message.contains("write an attribute of"),
        "{}",
        err.message
    );
}

#[test]
fn reading_attributes_of_a_missing_entry_is_empty_rather_than_a_failure() {
    let store = store_over(&FakeCredential::empty());

    let attributes = store.attributes().expect("no entry means no attributes");

    assert!(attributes.is_empty());
}
