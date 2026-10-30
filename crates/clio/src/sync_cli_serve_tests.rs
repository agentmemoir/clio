// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync key a `clio sync serve` process serves with, and
//! the refusals the serve start answers before it binds.
//!
//! ## Owns
//! - The key the peer serves with: the explicit key file, the recorded store,
//!   the keyless node, and the refusal when a configured key cannot be read
//! - The start's refusals: an anonymous bind outside dev mode, and a `banks`
//!   argument the serve cannot cover
//! - The start's own terms: the bound address, the banks, the TTL, and dev mode
//!
//! ## Does not own
//! - The key read itself (see `clio-config`'s `sync_key` and `sync_key_file`)
//! - The denial authority the started server is served with (see
//!   `sync_cli_denial_tests`)
//! - The verb, its flags, and its text view (`sync_cli`)
//!
//! ## Boundary
//!
//! Tests here break when the serve stops serving the key its custody route
//! resolves, when a key read failure stops refusing, or when the start stops
//! reporting the terms it bound. They never change any of it.
//!
//! Every environment is injected, so no test reads this machine's own key
//! store or its own sync key file.
//!
//! Keep this module focused on the served key and the start's terms.

use super::*;
use crate::cli_read_test_support::state;
use crate::sync_key_cli_test_support::{TempDir, env_of, stored_key};
use clio_sync::prelude::SyncCipherKey;
use serde_json::json;
use std::process::ExitCode;

/// A readable key file is the key the peer serves with: the served key is the
/// one that file holds, identified the way the wire identifies it.
#[test]
fn a_readable_key_file_is_the_key_the_peer_serves_with() {
    let dir = TempDir::new("serve-key-file");
    let env = env_of(vec![(
        clio_config::SYNC_KEY_FILE_ENV,
        dir.key().display().to_string(),
    )]);
    let recorded = stored_key(&env);

    let served = serve_sync_key(&env)
        .expect("a readable key file is served")
        .expect("the node holds a key");

    assert_eq!(
        served.key_id(),
        SyncCipherKey::from_secret(&recorded.base64()).key_id(),
        "the peer serves with the key the explicit file holds"
    );
}

/// With no key file named, the recorded store answers: the peer serves with the
/// key this node recorded, so the pushes it receives were sealed under a key it
/// holds.
#[test]
fn a_node_without_the_key_file_variable_serves_the_key_its_store_recorded() {
    let dir = TempDir::new("serve-recorded-key");
    let env = env_of(vec![(
        clio_config::KEY_FILE_ENV,
        dir.path().join("clio.key").display().to_string(),
    )]);
    assert_eq!(
        env(clio_config::SYNC_KEY_FILE_ENV),
        None,
        "no file is named"
    );
    let recorded = stored_key(&env);

    let served = serve_sync_key(&env)
        .expect("the recorded key is served")
        .expect("the node holds a key");

    assert_eq!(
        served.key_id(),
        SyncCipherKey::from_secret(&recorded.base64()).key_id(),
        "the peer serves with the key its recorded store holds"
    );
}

/// A node that recorded no key serves none: it cannot open a sealed row, so it
/// resolves no subject for one, which is how it served before keys existed.
#[test]
fn a_node_that_recorded_no_key_serves_none() {
    let dir = TempDir::new("serve-keyless");
    let env = env_of(vec![(
        clio_config::KEY_FILE_ENV,
        dir.path().join("clio.key").display().to_string(),
    )]);

    let served = serve_sync_key(&env).expect("a keyless node still serves");

    assert!(served.is_none(), "a node with no key serves no key");
}

/// A configured key file that cannot be read is refused rather than served
/// keyless: a serve process that silently lost its key would apply every erased
/// subject it was sent.
#[test]
fn a_key_file_that_cannot_be_read_refuses_the_serve() {
    let dir = TempDir::new("serve-key-missing");
    let env = env_of(vec![(
        clio_config::SYNC_KEY_FILE_ENV,
        dir.key().display().to_string(),
    )]);
    let path = dir.key().display().to_string();

    let error = serve_sync_key(&env).expect_err("an unreadable key file must not serve");

    assert_eq!(error.exit_code(), ExitCode::from(1), "{error:?}");
    assert!(
        error.message().contains(&path),
        "the refusal names the key file that fixes it: {}",
        error.message()
    );
}

/// A recorded key that cannot be read is refused the same way, so a damaged
/// store cannot turn a serve into an unguarded one.
#[test]
fn a_recorded_key_that_cannot_be_read_refuses_the_serve() {
    let dir = TempDir::new("serve-key-damaged");
    std::fs::write(dir.key(), b"short").expect("seed a damaged key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.key(), std::fs::Permissions::from_mode(0o600))
            .expect("restrict the seeded key");
    }
    let env = env_of(vec![(
        clio_config::KEY_FILE_ENV,
        dir.path().join("clio.key").display().to_string(),
    )]);

    let error = serve_sync_key(&env).expect_err("a damaged recorded key must not serve");

    assert_eq!(error.exit_code(), ExitCode::from(1), "{error:?}");
    assert!(
        error.message().contains("bytes; expected"),
        "the refusal says the key is the wrong length: {}",
        error.message()
    );
}

/// The start refuses an anonymous bind outside dev mode before it binds: the
/// caller must present auth material, and a downloaded peer is not a reason to
/// serve without it.
#[test]
fn a_serve_without_auth_material_outside_dev_is_refused() {
    let env = |_key: &str| -> Option<String> { None };

    let error = start_serve(&state(), &json!({ "bind": "127.0.0.1:0" }), &env)
        .err()
        .expect("an anonymous bind outside dev mode is refused");

    assert_eq!(error.exit_code(), ExitCode::from(1), "{error:?}");
    assert_eq!(error.code(), "forbidden", "{error:?}");
    assert!(
        error.message().contains("refusing anonymous bind"),
        "the refusal says what it refused: {}",
        error.message()
    );
}

/// The start reports the terms it bound: the address it listens on, the banks
/// it covers, the TTL a caller asked for, and dev mode.
#[test]
fn a_serve_reports_the_terms_it_bound() {
    let dir = TempDir::new("serve-terms");
    let journal = dir.path().join(clio_config::ERASURE_JOURNAL_FILE_NAME);
    let key_path = dir.key().display().to_string();
    let journal_path = journal.display().to_string();
    let env = move |key: &str| match key {
        clio_config::SYNC_KEY_FILE_ENV => Some(key_path.clone()),
        clio_config::ERASURE_JOURNAL_FILE_ENV => Some(journal_path.clone()),
        _ => None,
    };
    stored_key(&env);

    let (server, payload) = start_serve(
        &state(),
        &json!({ "dev": true, "bind": "127.0.0.1:0", "banks": ["demo"], "ttl_secs": 5 }),
        &env,
    )
    .expect("the serve process starts");

    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["banks"], json!(["demo"]), "{payload}");
    assert_eq!(payload["ttl_secs"], json!(5), "{payload}");
    assert_eq!(payload["dev"], json!(true), "{payload}");
    assert_eq!(
        payload["addr"].as_str(),
        Some(server.addr()),
        "the payload carries the address the server bound"
    );
    server.stop();
}
