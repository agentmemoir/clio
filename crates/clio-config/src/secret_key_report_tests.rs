// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the two ways Clio talks about
//! where the key is without holding it: `key_file_path` and `location`, plus the
//! promise that nothing printed ever contains key bytes.
//!
//! ## Owns
//! - `CLIO_KEY_FILE` precedence, including the blank-value case
//! - `location` reporting each store, and creating nothing
//! - The check that no rendered string contains hex or base64 key material
//! - The check that every keystore failure is summarised, never rendered
//!
//! ## Does not own
//! - Which key wins (see `secret_key_tests`)
//! - Test scaffolding (see `secret_key_support_tests`)
//!
//! ## Boundary
//!
//! The keystore messages here are built from `keyring` error values without an
//! entry, so no test reaches a real store. The point is the text: a rendered
//! `keyring` error can carry store contents, and this is where that is proved.
//!
//! Keep this module focused on reporting, never on holding the key.

use super::support_tests::*;
use super::*;
use crate::secret_key_source::{self, KeySource, write};
use crate::secret_key_store::{ENCRYPTION_KEY, classify, store_error};
use base64::Engine;

fn loc_err(result: Result<Option<KeyLocation>, AmError>) -> AmError {
    result.expect_err("expected a refusal")
}

/// Lower-case hex, the encoding a message would most plausibly leak the key in.
fn hex_of(key: &[u8; KEY_LEN]) -> String {
    use std::fmt::Write as _;
    key.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

// -------------------------------------------------------------- key_file_path

#[test]
fn the_key_file_env_override_wins_over_the_data_directory() {
    let get = env(&[
        ("CLIO_KEY_FILE", "/keys/clio.key"),
        ("CLIO_DATA_DIR", "/data"),
    ]);

    assert_eq!(key_file_path(&get), Some(PathBuf::from("/keys/clio.key")));
}

#[test]
fn the_key_file_defaults_to_the_data_directory() {
    assert_eq!(
        key_file_path(&env(&[("CLIO_DATA_DIR", "/data")])),
        Some(PathBuf::from("/data/clio.key"))
    );
    assert_eq!(
        key_file_path(&env(&[("XDG_DATA_HOME", "/xdg")])),
        Some(PathBuf::from("/xdg/clio/clio.key"))
    );
    assert_eq!(
        key_file_path(&env(&[("HOME", "/home/dev")])),
        Some(PathBuf::from("/home/dev/.local/share/clio/clio.key"))
    );
}

#[test]
fn a_blank_key_file_env_is_treated_as_unset() {
    for blank in ["", "   ", "\t"] {
        let get = env(&[("CLIO_KEY_FILE", blank), ("CLIO_DATA_DIR", "/data")]);

        assert_eq!(
            key_file_path(&get),
            Some(PathBuf::from("/data/clio.key")),
            "{blank:?} must fall through to the data directory"
        );
    }
}

#[test]
fn the_process_environment_is_read_the_ordinary_way() {
    // The one place the real environment is read. Everything else takes an
    // injected lookup, so this is the only path that can see the host, and it
    // reads a name no key path uses.
    assert_eq!(process_env("CLIO_KEY_FILE_ENV_THAT_DOES_NOT_EXIST"), None);
    assert_eq!(process_env("PATH").is_some(), std::env::var("PATH").is_ok());
}

// ------------------------------------------------------------------ location

#[test]
fn location_on_a_fresh_install_creates_nothing() {
    let store = FakeStore::empty();
    let file = key_file("loc-fresh");

    let found = location_with(&store, Some(file.as_path())).expect("location");

    assert_eq!(found, None);
    assert!(!file.exists(), "location must not write a key file");
    assert_eq!(
        store.get_calls(),
        0,
        "an absent record means the file store, so the keystore is not read"
    );
    assert_eq!(store.set_calls(), 0);
    assert_eq!(store.entry(), None);
}

#[test]
fn location_reports_the_keystore() {
    let store = FakeStore::holding(&random_key());
    let file = key_file("loc-keystore");
    record(&file, KeySource::Keystore);

    let found = location_with(&store, Some(file.as_path())).expect("location");

    assert_eq!(found, Some(KeyLocation::Keystore));
    assert!(!file.exists());
    assert_eq!(store.get_calls(), 1);
    assert_eq!(store.set_calls(), 0);
}

#[test]
fn location_reports_the_file_without_consulting_the_store() {
    let file = key_file("loc-file");
    write_key(&file, &random_key());
    let store = FakeStore::holding(&random_key());

    let found = location_with(&store, Some(file.as_path())).expect("location");

    assert_eq!(found, Some(KeyLocation::File(file)));
    assert_eq!(
        store.get_calls(),
        0,
        "a present file is the answer, so the store is not consulted"
    );
    assert_eq!(store.set_calls(), 0);
}

#[test]
fn location_reports_an_unreachable_store_as_an_error_not_as_no_key() {
    let file = key_file("loc-locked");
    record(&file, KeySource::Keystore);

    let err = loc_err(location_with(
        &FakeStore::unreadable("store is locked"),
        Some(file.as_path()),
    ));

    assert_eq!(
        err.code,
        ErrorCode::Internal,
        "a locked store must not read as 'no key yet'"
    );
    assert_eq!(err.message, "store is locked");
}

// ----------------------------------------------------------------- rendering

#[test]
fn key_locations_render_readably() {
    assert_eq!(KeyLocation::Keystore.to_string(), display_name());
    assert_eq!(
        KeyLocation::File(PathBuf::from("/keys/clio.key")).to_string(),
        "key file /keys/clio.key"
    );
}

#[test]
fn no_rendered_output_contains_key_material() {
    let keys = [random_key(), random_key()];

    for rendered in rendered_output(&keys) {
        for key in &keys {
            let hex = hex_of(key);
            let base64 = base64::engine::general_purpose::STANDARD.encode(key);
            assert!(!rendered.contains(&hex), "hex key leaked: {rendered}");
            assert!(!rendered.contains(&base64), "base64 key leaked: {rendered}");
        }
    }
}

#[test]
fn every_keystore_failure_summary_is_reachable_and_says_something() {
    // The wildcard arm of `classify` is the one arm that cannot be built from
    // outside the `keyring` crate, so it is the one arm no test reaches. Every
    // other arm is checked here, which is what keeps the wildcard from quietly
    // becoming the arm that renders an error object.
    let wanted = random_key();
    let failures = [
        keyring::Error::NoEntry,
        keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))),
        keyring::Error::PlatformFailure(Box::new(std::io::Error::other("bus down"))),
        keyring::Error::BadEncoding(wanted.to_vec()),
        keyring::Error::TooLong("service".to_owned(), 900),
        keyring::Error::Invalid("service".to_owned(), "empty".to_owned()),
    ];

    let mut summaries = Vec::new();
    for err in &failures {
        let message = store_error("read from", ENCRYPTION_KEY, err).message;

        assert!(message.contains(display_name()), "{message}");
        assert!(
            !message.contains(&base64::engine::general_purpose::STANDARD.encode(wanted)),
            "a rendered keystore error leaked the stored bytes: {message}"
        );
        summaries.push(classify(err));
    }

    summaries.sort_unstable();
    summaries.dedup();
    assert_eq!(
        summaries.len(),
        failures.len(),
        "each failure gets its own wording, so none is mistaken for another"
    );
}

/// Every operator-facing string this module can produce.
///
/// `keys[0]` sits in the keystore and `keys[1]` in the key file, and each store is
/// rendered in turn, so no message here is about bytes nothing else holds. The
/// record is set per case so each load follows the store it is exercising: the
/// default is the file store, and the keystore cases say so explicitly.
fn rendered_output(keys: &[[u8; KEY_LEN]; 2]) -> Vec<String> {
    let (keystore_key, file_key) = (&keys[0], &keys[1]);
    let file = key_file("render");
    let mut out = Vec::new();

    // The keystore first, with no file in play, and the record saying so.
    write(
        secret_key_source::marker_path(Some(&file)).as_deref(),
        KeySource::Keystore,
    )
    .expect("record");
    let from_keystore =
        load_or_create_with(&FakeStore::holding(keystore_key), Some(file.clone())).expect("store");
    out.push(format!("{from_keystore:?}"));
    out.push(KeyLocation::Keystore.to_string());
    out.push(KeyLocation::File(file.clone()).to_string());

    // Both stores populated is no longer a refusal. The operator chose one, so
    // the other is reported and ignored rather than treated as a contradiction.
    write_key(&file, file_key);
    // `file` is moved here because this is its last use in the function; the two
    // lines above still need to borrow it.
    out.push(format!(
        "{:?}",
        load_or_create_with(&FakeStore::holding(keystore_key), Some(file))
            .expect("the chosen store answers")
    ));

    // The file store, with an unreachable keystore that is never consulted. Its
    // own key file, because the record lives beside the key file and one record
    // per directory is the only way two stores can both be rendered here.
    let file_case = key_file("render-file");
    write(
        secret_key_source::marker_path(Some(&file_case)).as_deref(),
        KeySource::File,
    )
    .expect("record");
    // Moved rather than cloned: the path is not needed again, because the record
    // is already written and the rest of this case reads the loaded key, not the
    // path it came from.
    let from_file =
        load_or_create_with(&FakeStore::unreadable("locked"), Some(file_case)).expect("file");
    out.push(format!("{from_file:?}"));

    // A locked keystore is refused rather than redirected to a file, and that
    // refusal is an operator-facing string like any other.
    let locked_case = key_file("render-locked");
    write(
        secret_key_source::marker_path(Some(&locked_case)).as_deref(),
        KeySource::Keystore,
    )
    .expect("record");
    out.push(
        err_of(load_or_create_with(
            &FakeStore::unreadable("locked"),
            Some(locked_case),
        ))
        .to_string(),
    );

    // A key of the wrong length, in each store, is refused in both.
    let short = key_file("render-short");
    write_key(&short, &file_key[..5]);
    out.push(err_of(load_or_create_with(&FakeStore::empty(), Some(short))).to_string());

    out.push(
        err_of(load_or_create_with(
            &FakeStore::holding(&keystore_key.repeat(2)),
            None,
        ))
        .to_string(),
    );
    out.push(
        err_of(load_or_create_with(
            &FakeStore::empty(),
            Some(unreadable_file()),
        ))
        .to_string(),
    );
    out.push(err_of(load_or_create_with(&FakeStore::unreadable("no bus"), None)).to_string());
    out.push(
        err_of(load_or_create_with(
            &FakeStore::unreadable("no bus"),
            Some(blocked_key_file()),
        ))
        .to_string(),
    );
    // A store that reads but will not write still ends with a key, in the file.
    let fell_back = load_or_create_with(
        &FakeStore::refusing_writes("read-only"),
        Some(key_file("render-ro")),
    )
    .expect("fallback");
    out.push(format!("{fell_back:?}"));

    // The keystore's own messages, built without an entry so no store is
    // reached. `BadEncoding` and `Ambiguous` are the two upstream variants that
    // carry store contents, so both are rendered here.
    out.push(store_error("read from", ENCRYPTION_KEY, &keyring::Error::NoEntry).message);
    out.push(
        store_error(
            "read from",
            ENCRYPTION_KEY,
            &keyring::Error::BadEncoding(keystore_key.to_vec()),
        )
        .message,
    );
    out.push(
        store_error(
            "read from",
            ENCRYPTION_KEY,
            &keyring::Error::Ambiguous(vec![Box::new(SecretCredential(keys[0]))]),
        )
        .message,
    );
    out.push(
        store_error(
            "read from",
            ENCRYPTION_KEY,
            &keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))),
        )
        .message,
    );

    out
}

fn err_of(result: Result<LoadedKey, AmError>) -> AmError {
    result.expect_err("expected a refusal")
}

/// A platform credential whose `Debug` prints its secret, the way a real
/// Keychain item does. Rendering one of these is the leak this module avoids:
/// `Ambiguous` formats its credentials with `Debug`, so printing the error
/// would print the secret.
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
