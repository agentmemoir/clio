// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of setup's deployment-config
//! persistence: existing-value masking, validate-all-then-write-once, and the
//! backup-before-replace behavior.
//!
//! ## Owns
//! - Persistence behavior under test for `setup_config`
//!
//! ## Does not own
//! - Production persistence logic under test
//! - Docker orchestration or CLI parsing tests (see the sibling suites)
//!
//! ## Boundary
//!
//! Changes to setup persistence semantics should normally update tests here.
//! Changes to install-type selection should not require changes here.
//!
//! Keep this module focused on setup persistence tests.

use super::*;
use crate::setup_support_tests::{env_at, temp_dir};
use serde_json::json;

#[test]
fn watched_fields_include_the_paired_database_keys() {
    for field in ["backend", "store.database_url", "store.sqlite_path"] {
        assert!(
            WATCHED_FIELDS.contains(&field),
            "{field} must be watched so a switch writes or prunes it"
        );
    }
}

#[test]
fn write_persists_the_backend_and_the_address_as_one_plan() {
    let dir = temp_dir("paired");
    let path = dir.join("deployment.json");
    let address = "postgres://clio:s3cr3t-pw@127.0.0.1:34310/clio";
    let mut rt = Runtime::with_env(&env_at(&path));

    write(
        &mut rt,
        &path,
        &[
            ("backend".to_owned(), json!("postgres")),
            ("store.database_url".to_owned(), json!(address)),
        ],
    )
    .expect("paired write");

    // The file is sealed, so the address is checked through a fresh runtime
    // (the one the next command would build), not from the raw bytes.
    let reopened = Runtime::with_env(&env_at(&path));
    let effective = reopened.effective_raw();
    assert_eq!(effective["backend"], json!("postgres"));
    assert_eq!(effective["store"]["database_url"], json!(address));
    assert!(
        !std::fs::read_to_string(&path)
            .expect("read")
            .contains("s3cr3t-pw"),
        "the address is sealed on disk"
    );

    // The existing-value report must mask the address, which can carry a password.
    let report = existing_values(&rt);
    let reported = report
        .iter()
        .find(|value| value.path == "store.database_url")
        .expect("address reported");
    assert!(
        !reported.display.contains("s3cr3t-pw"),
        "the masked display leaked the password: {}",
        reported.display
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_stores_the_sqlite_path_for_a_file_plan() {
    let dir = temp_dir("file-plan");
    let path = dir.join("deployment.json");
    let file = dir.join("memories.db");
    let mut rt = Runtime::with_env(&env_at(&path));

    write(
        &mut rt,
        &path,
        &[
            ("backend".to_owned(), json!("sqlite")),
            (
                "store.database_url".to_owned(),
                json!(file.to_string_lossy()),
            ),
            (
                "store.sqlite_path".to_owned(),
                json!(file.to_string_lossy()),
            ),
        ],
    )
    .expect("file-plan write");

    let effective = Runtime::with_env(&env_at(&path)).effective_raw();
    assert_eq!(effective["backend"], json!("sqlite"));
    assert_eq!(
        effective["store"]["sqlite_path"],
        json!(file.to_string_lossy())
    );
    assert_eq!(
        effective["store"]["database_url"],
        json!(file.to_string_lossy())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_prunes_a_stale_sqlite_path_when_the_plan_has_none() {
    let dir = temp_dir("stale-path");
    let path = dir.join("deployment.json");
    let stale = "/tmp/am-setup-previous-store.db";
    std::fs::write(
        &path,
        format!(r#"{{"backend":"sqlite","store.sqlite_path":"{stale}"}}"#),
    )
    .expect("seed");
    let mut rt = Runtime::with_env(&env_at(&path));

    // Switching to Postgres writes the pair and omits the file path.
    write(
        &mut rt,
        &path,
        &[
            ("backend".to_owned(), json!("postgres")),
            (
                "store.database_url".to_owned(),
                json!("postgres://clio:pw@127.0.0.1:34310/clio"),
            ),
        ],
    )
    .expect("switch write");

    let effective = Runtime::with_env(&env_at(&path)).effective_raw();
    assert_eq!(effective["backend"], json!("postgres"));
    assert_ne!(
        effective["store"]["sqlite_path"],
        json!(stale),
        "the stale path must not survive a switch to Postgres"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn existing_values_are_masked_and_only_file_set_fields_report() {
    let dir = temp_dir("existing");
    let path = dir.join("deployment.json");
    // The api key sits at a secret path, so the seed goes through the save
    // path: a hand-written plaintext credential is refused by the loader.
    // Same shape as status_probe_tests.rs and db_agreement_tests.rs.
    let mut seed = clio_config::Runtime::with_deployment_file(&path);
    for (key, value) in [
        ("embed.provider", "openai"),
        ("credentials.embed_api_key", "sk-secret-1234567890"),
    ] {
        let clio_config::error::ToolResult::Ok { .. } =
            seed.config_set(key, json!(value), clio_config::ConfigScope::Deployment)
        else {
            panic!("config_set {key}");
        };
    }
    let rt = Runtime::with_env(&env_at(&path));

    let found = existing_values(&rt);
    let key = found
        .iter()
        .find(|v| v.path == "credentials.embed_api_key")
        .expect("key reported");
    assert!(key.display.contains("****"));
    assert!(!key.display.contains("sk-secret"));
    let provider = found
        .iter()
        .find(|v| v.path == "embed.provider")
        .expect("provider reported");
    assert_eq!(provider.display, "openai");
    // A field only present as a merge default is not "already configured".
    assert!(!found.iter().any(|v| v.path == "embed.model"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_persists_once_and_backs_up_the_previous_config() {
    let dir = temp_dir("write");
    let path = dir.join("nested").join("deployment.json");
    let mut rt = Runtime::with_env(&env_at(&path));

    let first = write(
        &mut rt,
        &path,
        &[
            ("embed.provider".to_owned(), json!("openai")),
            (
                "credentials.embed_api_key".to_owned(),
                json!("sk-first-1234567890"),
            ),
        ],
    )
    .expect("first write");
    assert_eq!(first.fields, 2);
    assert!(first.backup.is_none(), "nothing existed to back up");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(on_disk["embed.provider"], json!("openai"));

    // A second write preserves unrelated (unwatched) keys and backs up the
    // first file, but drops watched fields the new plan no longer writes.
    // The reseed goes through the save path because the api key is a secret
    // path; a hand-written plaintext credential is refused by the loader.
    // Same shape as status_probe_tests.rs and db_agreement_tests.rs.
    let mut seed = clio_config::Runtime::with_deployment_file(&path);
    for (key, value) in [
        ("log.level", "info"),
        ("embed.provider", "openai"),
        ("credentials.embed_api_key", "sk-first-1234567890"),
    ] {
        let clio_config::error::ToolResult::Ok { .. } =
            seed.config_set(key, json!(value), clio_config::ConfigScope::Deployment)
        else {
            panic!("config_set {key}");
        };
    }
    let mut rt2 = Runtime::with_env(&env_at(&path));
    let second = write(
        &mut rt2,
        &path,
        &[("rerank.provider".to_owned(), json!("cohere"))],
    )
    .expect("second write");
    let backup = second.backup.expect("backup created");
    assert!(backup.starts_with(path.parent().expect("parent").join("backups")));
    assert!(
        std::fs::read_to_string(&backup)
            .expect("read")
            .contains("openai")
    );
    let merged: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(merged["rerank.provider"], json!("cohere"));
    assert_eq!(merged["log.level"], json!("info"), "unwatched keys survive");
    assert!(
        merged.get("embed.provider").is_none(),
        "watched fields the new plan drops are pruned"
    );
    assert!(merged.get("credentials").is_none(), "{merged}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_reports_backup_failure_before_replacing() {
    let dir = temp_dir("backup-fail");
    // A directory at the config path exists but cannot be read as a file, so the
    // backup step fails and no replacement happens.
    let path = dir.join("config-dir");
    std::fs::create_dir_all(&path).expect("dir");
    let mut rt = Runtime::with_env(&env_at(&path));
    let err = write(
        &mut rt,
        &path,
        &[("embed.provider".to_owned(), json!("tei"))],
    )
    .expect_err("backup fails");
    assert!(err.contains("backup failed"), "{err}");
    assert!(path.is_dir(), "previous config path untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_reports_flush_failure_without_replacing() {
    let dir = temp_dir("flush-fail");
    // A file occupies the directory slot the config path needs, so the flush
    // cannot create parents and fails before replacing anything.
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, "not a directory").expect("blocker");
    let path = blocker.join("deployment.json");
    let mut rt = Runtime::with_env(&env_at(&path));
    let err = write(
        &mut rt,
        &path,
        &[("embed.provider".to_owned(), json!("openai"))],
    )
    .expect_err("flush fails");
    assert!(err.contains("failed to write"), "{err}");
    // Nothing was written and no backup was taken (nothing existed).
    assert!(!path.exists());
    assert!(!dir.join("backups").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_prunes_stale_credentials_when_the_install_type_drops_them() {
    let dir = temp_dir("prune");
    let path = dir.join("deployment.json");
    // A previous install type left hosted API keys in the file.
    std::fs::write(
        &path,
        r#"{"embed.provider":"openai","embed.model":"m1","credentials.embed_api_key":"sk-old-1234567890","embed.dims":1536}"#,
    )
    .expect("seed");
    let mut rt = Runtime::with_env(&env_at(&path));

    // A local install writes no credential or dims fields, so they are dropped.
    write(
        &mut rt,
        &path,
        &[
            ("embed.provider".to_owned(), json!("tei")),
            ("embed.url".to_owned(), json!("http://127.0.0.1:1")),
            ("embed.model".to_owned(), json!("m2")),
        ],
    )
    .expect("write");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert!(saved.get("credentials").is_none(), "{saved}");
    assert!(saved.get("embed.dims").is_none(), "{saved}");
    assert_eq!(saved["embed.provider"], json!("tei"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn write_rejects_invalid_value_and_leaves_previous_config_intact() {
    let dir = temp_dir("invalid");
    let path = dir.join("deployment.json");
    std::fs::write(&path, r#"{"embed.provider":"tei"}"#).expect("seed");
    let mut rt = Runtime::with_env(&env_at(&path));

    let err = write(
        &mut rt,
        &path,
        &[("embed.provider".to_owned(), json!("bogus-provider"))],
    )
    .expect_err("invalid provider rejected");
    assert!(err.contains("embed.provider"), "names the field: {err}");
    // The file is untouched and no backup was taken.
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        r#"{"embed.provider":"tei"}"#
    );
    assert!(!path.parent().expect("parent").join("backups").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
