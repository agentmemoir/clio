// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared helpers behind the deployment
//! overlay sealing tests, plus the save-bytes and round-trip tests themselves.
//!
//! ## Owns
//! - The scratch directory, the test key, and the saved-overlay helpers
//! - The claim that the written file holds no readable secret
//! - The round trip through `Runtime`: save, reload, `config_get`, `effective_raw`
//!
//! ## Does not own
//! - The load refusal shapes (see `seal_gate_tests`)
//! - The save recovery and the refused-overwrite guard (see `seal_write_tests`)
//! - The envelope's own failures (see `secret_seal_tests`)
//!
//! ## Boundary
//!
//! Changes to the shared sealing helpers should normally require changes here.
//! Changes to a refusal shape should not require changes here.
//!
//! Keep this file focused on shared helpers and the save-bytes round trip.
//! Every test installs the key explicitly, so none reaches the machine's key
//! store.

use super::*;
use crate::error::ToolResult;
use crate::secret_seal::test_key;
use serde_json::json;

/// The key every sealing test seals under.
pub(super) const KEY: [u8; crate::secret_key::KEY_LEN] = [0x33; crate::secret_key::KEY_LEN];

/// The three values a setup run writes, plus one value it does not.
pub(super) const API_KEY: &str = "sk-live-DEADBEEF0123456789";
pub(super) const RERANK_KEY: &str = "sk-rerank-CAFEBABE9876543210";
pub(super) const DATABASE_URL: &str = "postgres://clio:hunter2@127.0.0.1:5432/clio";
pub(super) const LOG_LEVEL: &str = "debug";

/// A scratch directory that cleans itself up when the guard drops.
pub(super) struct Scratch(PathBuf);

impl Scratch {
    pub(super) fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("clio-persist-seal-{tag}-{nanos}"));
        fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    pub(super) fn config(&self) -> PathBuf {
        self.0.join("deployment.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A saved overlay holding every value the setup paths write.
pub(super) fn save_secrets(path: &Path) {
    let mut overlay = Overlay::new();
    overlay.insert("credentials.embed_api_key".into(), json!(API_KEY));
    overlay.insert("credentials.rerank_api_key".into(), json!(RERANK_KEY));
    overlay.insert("store.database_url".into(), json!(DATABASE_URL));
    overlay.insert("log.level".into(), json!(LOG_LEVEL));
    save_overlay_file(path, &overlay).expect("save a sealed overlay");
}

pub(super) fn file_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read the written file")
}

#[test]
fn the_written_file_holds_no_readable_secret() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("no-plaintext");
    let path = scratch.config();
    save_secrets(&path);

    let text = file_text(&path);
    for secret in [API_KEY, RERANK_KEY, DATABASE_URL] {
        assert!(!text.contains(secret), "the file leaked {secret}");
        assert!(
            !text.contains(&secret[..10]),
            "the file leaked the head of {secret}"
        );
        assert!(
            !text.contains(&secret[secret.len() - 10..]),
            "the file leaked the tail of {secret}"
        );
    }
    // The value that is not a secret is still readable, which is what makes the
    // sealed ones meaningful: the file was written, just not in the clear.
    assert!(text.contains(LOG_LEVEL));
}

#[test]
fn every_sealed_path_appears_as_an_envelope() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("envelopes");
    let path = scratch.config();
    save_secrets(&path);

    let text = file_text(&path);
    assert_eq!(
        text.matches(crate::secret_seal::SEALED_PREFIX).count(),
        3,
        "one envelope per secret, and none for log.level: {text}"
    );
}

#[test]
fn a_reload_hands_back_plaintext() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("reload");
    let path = scratch.config();
    save_secrets(&path);

    let reloaded = load_overlay_file(&path).expect("reload");
    assert_eq!(
        reloaded.get("credentials.embed_api_key"),
        Some(&json!(API_KEY))
    );
    assert_eq!(
        reloaded.get("store.database_url"),
        Some(&json!(DATABASE_URL))
    );
}

#[test]
fn the_runtime_hands_back_plaintext_through_both_read_tools() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("runtime");
    let path = scratch.config();
    save_secrets(&path);

    let rt = crate::config::Runtime::with_deployment_file(&path);
    assert_eq!(
        rt.effective_raw()["credentials"]["embed_api_key"],
        json!(API_KEY)
    );
    assert_eq!(
        rt.effective_raw()["store"]["database_url"],
        json!(DATABASE_URL)
    );

    let got = rt.config_get(Some("store.database_url"));
    let ToolResult::Ok { data, .. } = got else {
        panic!("config_get store.database_url");
    };
    let text = serde_json::to_string(&data.value).expect("serialize the get result");
    assert!(text.contains("****"), "the get result is masked: {text}");
    assert!(
        !text.contains("hunter2"),
        "the password is not in the output"
    );
}
