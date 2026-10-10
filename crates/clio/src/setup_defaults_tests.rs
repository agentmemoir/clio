// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the defaults: that a stored value
//! becomes the question's default, and that an unparsable address is ignored
//! rather than fatal.
//!
//! ## Owns
//! - Defaults behavior under test for `setup_defaults`
//!
//! ## Does not own
//! - The questions the defaults feed (see `setup_flows_db_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to what a question defaults to should normally update tests here.
//! Changes to which questions are asked should not require changes here.
//!
//! Keep this module focused on reading the last value out of a document.

use super::*;
use serde_json::json;

const STORED: &str =
    "postgres://clio:s3cret@db.example.com:5555/clio?sslmode=require&application_name=me";

#[test]
fn a_fresh_install_has_no_defaults_at_all() {
    let defaults = Defaults::from_effective(&json!({}));
    assert_eq!(defaults, Defaults::empty());
    assert_eq!(defaults.host, None);
    assert_eq!(defaults.sqlite_path, None);
}

#[test]
fn sync_is_in_play_only_when_the_effective_config_does_not_omit_it() {
    assert!(
        !Defaults::from_effective(&json!({})).sync_in_play,
        "an absent pointer is the single-host default"
    );
    assert!(
        !Defaults::from_effective(&json!({ "sync": { "omit": true } })).sync_in_play,
        "an omitted sync is single-host"
    );
    assert!(
        Defaults::from_effective(&json!({ "sync": { "omit": false } })).sync_in_play,
        "sync.omit=false is the multi-host case the question is for"
    );
}

#[test]
fn a_stored_address_fills_every_field_of_the_typed_route() {
    let defaults = Defaults::from_effective(&json!({ "store": { "database_url": STORED } }));
    assert_eq!(defaults.address.as_deref(), Some(STORED));
    assert_eq!(defaults.host.as_deref(), Some("db.example.com"));
    assert_eq!(defaults.port.as_deref(), Some("5555"));
    assert_eq!(defaults.name.as_deref(), Some("clio"));
    assert_eq!(defaults.user.as_deref(), Some("clio"));
    assert_eq!(defaults.ssl.as_deref(), Some("require"));
    assert!(
        defaults.has_password,
        "a stored password is a fact, not a value"
    );
}

#[test]
fn a_stored_address_reads_back_the_advanced_settings_it_carries() {
    let defaults = Defaults::from_effective(&json!({ "store": { "database_url": STORED } }));
    assert_eq!(
        defaults.advanced,
        vec![("application_name".to_owned(), "me".to_owned())]
    );
}

#[test]
fn an_unparsable_stored_address_is_ignored_rather_than_fatal() {
    // A config someone edited by hand is exactly the install the wizard is most
    // useful for, so the parse failing must not stop the run.
    let defaults = Defaults::from_effective(&json!({
        "store": { "database_url": "this is not a connection string" }
    }));
    assert_eq!(
        defaults.address.as_deref(),
        Some("this is not a connection string")
    );
    assert_eq!(defaults.host, None, "no host can be read from it");
    assert_eq!(defaults.port, None);
    assert!(!defaults.has_password);
}

#[test]
fn a_stored_backend_becomes_the_database_choice() {
    let sqlite = Defaults::from_effective(&json!({ "backend": "sqlite" }));
    assert_eq!(sqlite.database, Some(DatabaseChoice::Sqlite));
    // A stored backend cannot say which Postgres situation it was, because both
    // write `postgres`, so the address is the only thing that settles it.
    let postgres = Defaults::from_effective(&json!({ "backend": "postgres" }));
    assert_eq!(postgres.database, Some(DatabaseChoice::Existing));
}

#[test]
fn a_stored_sqlite_path_becomes_the_path_default() {
    let defaults =
        Defaults::from_effective(&json!({ "store": { "sqlite_path": "/data/clio.db" } }));
    assert_eq!(defaults.sqlite_path, Some(PathBuf::from("/data/clio.db")));
}

#[test]
fn the_service_values_become_their_defaults() {
    let defaults = Defaults::from_effective(&json!({
        "embed": { "url": "https://embed.example.test", "model": "m", "provider": "openai" },
        "rerank": { "url": "https://rerank.example.test", "model": "r", "provider": "cohere" },
        "credentials": { "embed_api_key": "sk-x", "rerank_api_key": "" }
    }));
    assert_eq!(
        defaults.embed_url.as_deref(),
        Some("https://embed.example.test")
    );
    assert_eq!(defaults.embed_model.as_deref(), Some("m"));
    assert_eq!(defaults.embed_provider.as_deref(), Some("openai"));
    assert_eq!(
        defaults.rerank_url.as_deref(),
        Some("https://rerank.example.test")
    );
    assert!(defaults.has_embed_key);
    // An empty string counts as absent: showing a default the operator cannot see
    // is worse than asking.
    assert!(!defaults.has_rerank_key);
}

#[test]
fn a_stored_address_never_leaks_its_password_through_the_defaults() {
    let defaults = Defaults::from_effective(&json!({ "store": { "database_url": STORED } }));
    // The address is kept as written because the paste route offers it back, but
    // the only password fact the defaults carry is the boolean.
    assert!(defaults.has_password);
    assert!(
        !format!("{defaults:?}").contains("s3cret-value"),
        "the defaults must not carry a password value"
    );
}

#[test]
fn a_value_of_the_wrong_type_is_treated_as_absent() {
    let defaults = Defaults::from_effective(&json!({
        "backend": 7,
        "store": { "database_url": 9, "sqlite_path": null }
    }));
    assert_eq!(defaults.database, None);
    assert_eq!(defaults.address, None);
    assert_eq!(defaults.sqlite_path, None);
}

#[test]
fn a_password_of_only_whitespace_counts_as_absent() {
    let defaults = Defaults::from_effective(&json!({
        "store": { "database_url": "postgres://clio:@db.example.com:5432/clio" }
    }));
    assert!(!defaults.has_password);
}

#[test]
fn the_password_presence_check_survives_a_percent_encoded_password() {
    let defaults = Defaults::from_effective(&json!({
        "store": { "database_url": "postgres://clio:p%40ss@db.example.com:5432/clio" }
    }));
    assert!(
        defaults.has_password,
        "an encoded password is still a password"
    );
}
