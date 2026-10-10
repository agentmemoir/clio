// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the deployment overlay's save side: what a
//! write seals, what a read leaves alone, and what a write must refuse.
//!
//! ## Owns
//! - Recovery by setting the value again, so the current save path seals it
//! - The rule that a read-only command leaves the file byte-identical
//! - The rule that a deployment write refuses to replace a refused file
//!
//! ## Does not own
//! - The shared helpers (see `seal_tests`)
//! - The load refusal shapes (see `seal_gate_tests`)
//! - Durable file layout and permissions (see `persist_tests`)
//!
//! ## Boundary
//!
//! Changes to the save path or its overwrite guard should normally require
//! changes here. Changes to a refusal shape should not require changes here.
//!
//! Keep this file focused on saves, reads that must not write, and refused
//! writes. Every test installs the key explicitly, so none reaches the
//! machine's key store.

use super::seal_tests::{API_KEY, DATABASE_URL, KEY, LOG_LEVEL, Scratch, file_text, save_secrets};
use super::*;
use crate::error::ToolResult;
use crate::secret_seal::test_key;
use serde_json::json;

#[test]
fn resealing_through_the_current_save_path_restores_effective_equality() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("reseal");
    let path = scratch.config();
    // The recovery for a refused file: set the values again, so the existing
    // save path seals them. Nothing here writes plaintext to the file.
    let mut rt = crate::config::Runtime::with_deployment_file(&path);
    for (path_name, value) in [
        ("credentials.embed_api_key", json!(API_KEY)),
        ("log.level", json!(LOG_LEVEL)),
    ] {
        let ToolResult::Ok { .. } = rt.config_set(path_name, value, crate::ConfigScope::Deployment)
        else {
            panic!("config_set {path_name} at deployment scope");
        };
    }

    // The persistent form is sealed and owner-only, and the ordinary value
    // stays plain: only the secret path carries an envelope.
    let text = file_text(&path);
    assert!(!text.contains(API_KEY), "the file leaked the key: {text}");
    assert_eq!(text.matches(crate::secret_seal::SEALED_PREFIX).count(), 1);
    assert!(
        text.contains(LOG_LEVEL),
        "an ordinary value is not a secret"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the saved config must be owner-only");
    }

    // Effective equality is checked in memory: the reloaded runtime hands back
    // exactly the values that were set.
    let reloaded = crate::config::Runtime::with_deployment_file(&path);
    assert_eq!(
        reloaded.effective_raw()["credentials"]["embed_api_key"],
        json!(API_KEY)
    );
    assert_eq!(reloaded.effective_raw()["log"]["level"], json!(LOG_LEVEL));
}

#[test]
fn reading_never_rewrites_the_file() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("read-only");
    let path = scratch.config();
    save_secrets(&path);
    let before = file_text(&path);
    let before_meta = fs::metadata(&path).expect("metadata").len();

    for _ in 0..3 {
        let rt = crate::config::Runtime::with_deployment_file(&path);
        assert_eq!(
            rt.effective_raw()["credentials"]["embed_api_key"],
            json!(API_KEY)
        );
    }

    assert_eq!(file_text(&path), before, "a read rewrote the file");
    assert_eq!(fs::metadata(&path).expect("metadata").len(), before_meta);
    assert!(
        !path
            .parent()
            .expect("parent")
            .join(BACKUP_DIR_NAME)
            .exists(),
        "a read created a backup directory"
    );
}

#[test]
fn a_deployment_write_refuses_to_replace_a_refused_file() {
    // Boot leaves a refused file unloaded (and warns through the loader); the
    // next deployment write must refuse with the same re-seal advice instead
    // of saving an overlay that has lost the legacy secret.
    let scratch = Scratch::new("refused-overwrite");
    let path = scratch.config();
    let legacy = serde_json::to_string_pretty(
        &json!({"log.level": LOG_LEVEL, "store.database_url": DATABASE_URL}),
    )
    .expect("serialize");
    fs::write(&path, &legacy).expect("write a legacy file");
    let mut rt = crate::config::Runtime::with_deployment_file(&path);
    let ToolResult::Err { code, message, .. } =
        rt.config_set("log.level", json!("warn"), crate::ConfigScope::Deployment)
    else {
        panic!("a refused file must not be overwritten");
    };
    assert_eq!(code, ErrorCode::ConfigCorrupt);
    assert!(
        message.contains("store.database_url"),
        "names the refused path: {message}"
    );
    assert!(
        message.contains("seal"),
        "points at re-sealing through the save path: {message}"
    );
    assert_eq!(
        file_text(&path),
        legacy,
        "a refused write never replaces the file"
    );
}
