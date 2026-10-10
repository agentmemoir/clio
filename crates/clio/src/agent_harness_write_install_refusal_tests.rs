// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the refusal tests of the direct-write
//! install merge: the parse refusal that keeps a file byte-identical and
//! leaves no debris, and the refusal of a non-object server map.
//!
//! ## Owns
//! - The `assert_refused_untouched` fixture and one test per refusal
//!   behavior of `agent_harness_write_install`
//!
//! ## Does not own
//! - The install implementation under test (`agent_harness_write_install`)
//! - The shared fixtures in `agent_harness_write_install_tests`
//!
//! ## Boundary
//!
//! Changes to refusal behavior should normally require changes here.
//! Changes to creation, merge, reinstall, uninstall, or permission behavior
//! should not require changes here.
//!
//! Keep this module focused on the direct-write refusal tests.

use super::tests::{
    Scratch, argv, backup_siblings, copilot_profile_config, cursor_config, siblings, write_fixture,
};
use super::*;
use std::fs;

/// A refusal keeps the file byte-identical, leaves no debris, names the
/// manual fragment and the offending line's number, and never copies the
/// line's bytes out of the file.
fn assert_refused_untouched(scratch: &Scratch, config: &DirectWriteConfig) {
    let bytes = b"{ // comment\n\"placeholder\": true\n}\n";
    write_fixture(config, bytes);

    let err = install_server_from_file(config, "clio", &argv(scratch, "clio-bin"))
        .expect_err("an unparseable file is refused");

    let WriteError::Unparseable { path, snippet } = err else {
        panic!("expected Unparseable, got {err:?}");
    };
    assert_eq!(path, config.path);
    assert!(
        snippet.contains(config.top_level_key)
            && snippet.contains("clio-bin")
            && snippet.contains("\"stdio\"")
            && snippet.contains("(offending line: 1)"),
        "the refusal keeps the manual fragment and the offending line's number: {snippet}"
    );
    assert!(
        !snippet.contains("// comment"),
        "the refusal must not copy the offending line's bytes out of the user's config: {snippet}"
    );
    assert_eq!(
        fs::read(&config.path).expect("file can be read"),
        bytes,
        "the bytes must not change"
    );
    let dir = config.path.parent().expect("config has a parent");
    assert_eq!(
        siblings(dir),
        vec![config.path.clone()],
        "only the config remains: no backup or temp debris"
    );
}

#[test]
fn cursor_commented_file_is_refused_untouched() {
    let scratch = Scratch::new("cursor-refuse");
    assert_refused_untouched(&scratch, &cursor_config(&scratch));
}

#[test]
fn copilot_profile_commented_file_is_refused_untouched() {
    let scratch = Scratch::new("copilot-refuse");
    assert_refused_untouched(&scratch, &copilot_profile_config(&scratch));
}

#[test]
fn a_non_object_server_map_is_refused_without_a_rewrite() {
    let scratch = Scratch::new("cursor-shape");
    let config = cursor_config(&scratch);
    let original = serde_json::json!({"mcpServers": []});
    write_fixture(&config, original.to_string());
    let before = fs::read(&config.path).expect("file can be read");

    let err = install_server_from_file(&config, "clio", &argv(&scratch, "clio-bin"))
        .expect_err("a non-object server map is refused");

    assert!(
        matches!(err, WriteError::Io { .. }),
        "expected Io, got {err:?}"
    );
    assert_eq!(
        fs::read(&config.path).expect("file can be read"),
        before,
        "the file must not change"
    );
    let dir = config.path.parent().expect("config has a parent");
    assert!(backup_siblings(dir).is_empty(), "no backup should be made");
}
