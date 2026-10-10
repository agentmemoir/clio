// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures used by the MCP sync
//! tool tests: in-memory state construction, a per-test temp directory for
//! key files, and the never-leak assertion over a response tree.
//!
//! ## Owns
//! - The in-memory `McpState` fixture and the temp key directory guard
//! - The assertion that a response tree carries no key material
//!
//! ## Does not own
//! - The sync tool assertions themselves (see `sync_tools_tests` and
//!   `sync_tools_key_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use std::sync::Arc;

pub(crate) fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions {
        backend: "sqlite".into(),
        database_url: "sqlite::memory:".into(),
        default_bank: "bank-mcp".into(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path: None,
    })
    .expect("mcp state")
}

/// A directory unique per call, removed when the guard drops.
///
/// The name carries the process id and a nanosecond stamp, so two tests in one
/// process and two processes on one machine cannot collide.
pub(crate) struct TempDir(std::path::PathBuf);

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let path =
            std::env::temp_dir().join(format!("clio-{label}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).expect("the temp directory can be created");
        Self(path)
    }

    pub(crate) fn key(&self) -> std::path::PathBuf {
        self.0.join(clio_config::SYNC_KEY_FILE_NAME)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Assert that `result` carries neither the stored key nor any prefix of it.
///
/// `fingerprint` is passed in so a caller can state the one 16-character value it
/// does expect, and it is skipped in the prefix scan: the fingerprint is derived
/// from the key, so a 16-character prefix of that base64 could equal it by
/// coincidence and fail for a reason that is not a leak.
pub(crate) fn assert_no_key_material(result: &serde_json::Value, fingerprint: &str, key: &str) {
    let rendered = result.to_string();

    assert!(!rendered.contains("\"key\""), "{rendered}");
    for name in ["key_b64", "secret", "key_bytes", "sync_key"] {
        assert!(
            !rendered.contains(name),
            "a leaked field {name}: {rendered}"
        );
    }
    assert!(!key.is_empty(), "the premise: a key is stored");
    assert!(!rendered.contains(key), "the whole key leaked: {rendered}");
    for len in [4_usize, 8, 12, 16, 20, 24] {
        let prefix = &key[..len];
        if prefix == fingerprint {
            continue;
        }
        assert!(
            !rendered.contains(prefix),
            "a {len}-character prefix of the key leaked: {prefix} in {rendered}"
        );
    }
}
