// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the deployment overlay's load gate: an
//! unsealed string at a sealed path is refused, whatever shape the file takes.
//!
//! ## Owns
//! - The flat-file refusal and what its message names and never echoes
//! - The mixed flat-and-nested refusal and the sealed mixed shape that loads
//! - The nested-object refusal on the flattened dot-path
//! - The wrong-key refusal and its re-seal advice
//! - The non-secret edge cases the gate must leave alone
//!
//! ## Does not own
//! - The shared helpers (see `seal_tests`)
//! - The save recovery and the refused-overwrite guard (see `seal_write_tests`)
//! - The envelope's own failures (see `secret_seal_tests`)
//!
//! ## Boundary
//!
//! Changes to the flattened load gate should normally require changes here.
//! Changes to the save path should not require changes here.
//!
//! Keep this file focused on the load gate as seen through the overlay file.

use super::seal_tests::{API_KEY, DATABASE_URL, KEY, LOG_LEVEL, Scratch, file_text};
use super::*;
use crate::secret_seal::test_key;
use serde_json::json;

#[test]
fn an_unsealed_secret_in_a_real_deployment_file_is_refused_without_echoing_it() {
    let scratch = Scratch::new("unsealed");
    let path = scratch.config();
    // What a hand edit or a pre-sealing writer leaves behind: a readable
    // secret at a path Clio stores sealed, next to an ordinary value.
    fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "credentials.embed_api_key": API_KEY,
            "log.level": LOG_LEVEL,
        }))
        .expect("serialize"),
    )
    .expect("write a legacy file");
    let before = file_text(&path);

    let error = load_overlay_file(&path).expect_err("an unsealed secret must not load");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("credentials.embed_api_key"),
        "the message names the dot-path: {}",
        error.message
    );
    assert!(
        error.message.contains(&path.display().to_string()),
        "the message names the file: {}",
        error.message
    );
    assert!(
        !error.message.contains(&API_KEY[..10]),
        "the message never echoes the value: {}",
        error.message
    );
    assert!(
        error.message.contains("seal") && !error.message.contains("edit"),
        "the advice is to re-seal through the save path, never hand-edit: {}",
        error.message
    );

    // A refusal reads the file; it never rewrites the file it refused.
    assert_eq!(file_text(&path), before, "the load rewrote the file");
}

#[test]
fn a_sealed_value_that_cannot_be_opened_fails_the_load_and_names_the_path() {
    let scratch = Scratch::new("wrong-key");
    let path = scratch.config();
    // Sealed under one key, read under another: the state a restored backup
    // meets when the Clio key is not the one it was written with.
    test_key::use_key([0x44; crate::secret_key::KEY_LEN]);
    let mut overlay = Overlay::new();
    overlay.insert("store.database_url".into(), json!(DATABASE_URL));
    save_overlay_file(&path, &overlay).expect("seal under the first key");
    test_key::use_key([0x55; crate::secret_key::KEY_LEN]);

    let error = load_overlay_file(&path).expect_err("a wrong key must not load");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("store.database_url"),
        "the message names the dot-path: {}",
        error.message
    );
    assert!(
        error.message.contains(&path.display().to_string()),
        "the message names the file: {}",
        error.message
    );
    assert!(
        !error.message.contains("hunter2"),
        "the message never echoes the value: {}",
        error.message
    );
    // The recovery advice names the tooling that seals on save, never a
    // hand-edit of the file's plaintext.
    assert!(
        error.message.contains("tooling"),
        "the advice is to re-seal through the save path: {}",
        error.message
    );
    assert!(
        !error.message.contains("edit"),
        "the advice never says to hand-edit the value: {}",
        error.message
    );
}

#[test]
fn a_mixed_flat_and_nested_file_hides_no_plaintext_secret() {
    // A dotted key next to nested objects: the old flat heuristic left the
    // nested objects unflattened, so the sealed gate missed them entirely.
    let scratch = Scratch::new("mixed-gate");
    let path = scratch.config();
    fs::write(&path, serde_json::to_string_pretty(&json!({"log.level": LOG_LEVEL, "credentials": {"embed_api_key": API_KEY}, "store": {"database_url": DATABASE_URL}})).expect("serialize")).expect("write a mixed file");
    let before = file_text(&path);
    let error = load_overlay_file(&path).expect_err("mixed plaintext must not load");
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        error.message.contains("credentials.embed_api_key"),
        "names the flattened path: {}",
        error.message
    );
    assert!(
        !error.message.contains(&API_KEY[..10]),
        "never echoes the value: {}",
        error.message
    );
    assert!(
        !error.message.contains("hunter2"),
        "never echoes the value: {}",
        error.message
    );
    assert_eq!(
        file_text(&path),
        before,
        "a refusal never rewrites the file"
    );
    // The same mixed shape with envelopes loads and opens, so the fix gates
    // without breaking valid files.
    test_key::use_key(KEY);
    let sealed_api = crate::secret_seal::seal(API_KEY).expect("seal");
    let sealed_url = crate::secret_seal::seal(DATABASE_URL).expect("seal");
    fs::write(&path, serde_json::to_string_pretty(&json!({"log.level": LOG_LEVEL, "credentials": {"embed_api_key": sealed_api}, "store": {"database_url": sealed_url}})).expect("serialize")).expect("write a sealed mixed file");
    let reloaded = load_overlay_file(&path).expect("a sealed mixed file loads");
    assert_eq!(
        reloaded.get("credentials.embed_api_key"),
        Some(&json!(API_KEY))
    );
    assert_eq!(
        reloaded.get("store.database_url"),
        Some(&json!(DATABASE_URL))
    );
    assert_eq!(reloaded.get("log.level"), Some(&json!(LOG_LEVEL)));
}

#[test]
fn a_nested_object_file_meets_the_same_gate_as_a_flat_one() {
    let scratch = Scratch::new("nested");
    let path = scratch.config();
    fs::write(
        &path,
        serde_json::to_string_pretty(&json!({
            "credentials": { "embed_api_key": API_KEY },
            "log": { "level": LOG_LEVEL },
        }))
        .expect("serialize"),
    )
    .expect("write a nested file");

    // The gate runs on the flattened dot-path, so nesting hides nothing.
    let error = load_overlay_file(&path).expect_err("an unsealed nested secret must not load");
    assert!(
        error.message.contains("credentials.embed_api_key"),
        "the message names the flattened dot-path: {}",
        error.message
    );
}

#[test]
fn a_knob_that_merely_sounds_like_a_secret_stays_readable() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("knobs");
    let path = scratch.config();
    let mut overlay = Overlay::new();
    // Both names contain `token`, so the masking rule matches them. Neither is a
    // secret, and both have to stay numbers in the file: a knob that came back
    // as a sealed string would silently stop taking effect.
    overlay.insert("history.lesson_max_tokens".into(), json!(40));
    overlay.insert("injection.budget_tokens".into(), json!(2048));
    save_overlay_file(&path, &overlay).expect("save");

    let text = file_text(&path);
    assert!(!text.contains(crate::secret_seal::SEALED_PREFIX), "{text}");
    let reloaded = load_overlay_file(&path).expect("reload");
    assert_eq!(reloaded.get("history.lesson_max_tokens"), Some(&json!(40)));
    assert_eq!(reloaded.get("injection.budget_tokens"), Some(&json!(2048)));
}

#[test]
fn a_non_string_leaf_at_a_sealed_path_keeps_its_type() {
    test_key::use_key(KEY);
    let scratch = Scratch::new("non-string");
    let path = scratch.config();
    let mut overlay = Overlay::new();
    overlay.insert("credentials.api_key".into(), json!(null));
    save_overlay_file(&path, &overlay).expect("save");

    let text = file_text(&path);
    assert!(!text.contains(crate::secret_seal::SEALED_PREFIX), "{text}");
    assert_eq!(
        load_overlay_file(&path)
            .expect("reload")
            .get("credentials.api_key"),
        Some(&json!(null))
    );
}
