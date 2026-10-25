// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the preview's presence check: what
//! the run announces it will download, and when it announces nothing.
//!
//! ## Owns
//! - `preview_line` behavior under test for `setup_extract_model`
//!
//! ## Does not own
//! - The download itself, and its progress lines (see `setup_extract_model_tests`)
//! - The rendered preview around the line (see `setup_preview_tests`)
//!
//! ## Boundary
//!
//! Changes to what the preview promises about the download should normally update
//! tests here. Changes to what a download writes should not.
//!
//! Keep this module focused on the promise, not on the transfer.

use super::*;

/// An environment that names no model of its own, so the shipped one is meant.
fn no_env(_key: &str) -> Option<String> {
    None
}

#[test]
fn the_preview_announces_the_download_when_the_model_is_absent() {
    let dir = temp_dir("preview-absent");
    let root = dir.to_string_lossy().into_owned();

    let line = preview_line(&answers(Some(ServiceRole::Local)), Some(&root), &no_env)
        .expect("an absent model is announced");

    assert!(
        line.starts_with("numind.NuExtract-tiny-v1.5.Q4_K_M.gguf (397 MB)"),
        "the line names the file and the size the Hub advertises: {line}"
    );
    assert!(
        line.contains("from the Hugging Face Hub"),
        "the line says where the bytes come from: {line}"
    );
    assert!(
        line.contains("before it writes the config"),
        "the line says when the download happens: {line}"
    );
    assert!(
        !dir.join("models").exists(),
        "a presence check may not create the directory it looks in"
    );
}

#[test]
fn a_model_already_on_the_host_is_not_announced() {
    let dir = temp_dir("preview-present");
    let root = dir.to_string_lossy().into_owned();
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models");
    std::fs::write(models.join(GGUF_FILE_NAME), b"stub").expect("write the model");

    let line = preview_line(&answers(Some(ServiceRole::Local)), Some(&root), &no_env);

    assert_eq!(line, None, "there is nothing to announce");
}

#[test]
fn an_extract_role_that_is_not_local_announces_nothing() {
    let dir = temp_dir("preview-not-local");
    let root = dir.to_string_lossy().into_owned();

    for role in [ServiceRole::Skip, ServiceRole::Hosted] {
        let line = preview_line(&answers(Some(role)), Some(&root), &no_env);
        assert_eq!(line, None, "a {role:?} extract downloads nothing");
    }
}

#[test]
fn an_extract_role_nobody_chose_announces_nothing() {
    // A run that never reached the question has no extract role at all, and a line
    // promising a download it cannot describe would be a guess.
    let dir = temp_dir("preview-unanswered");
    let root = dir.to_string_lossy().into_owned();

    assert_eq!(
        preview_line(&answers(None), Some(&root), &no_env),
        None,
        "no answer means no announcement"
    );
}

#[test]
fn the_announced_name_is_the_one_the_compose_environment_chose() {
    let dir = temp_dir("preview-named");
    let root = dir.to_string_lossy().into_owned();
    let env = |key: &str| (key == "CLIO_EXTRACT_GGUF").then(|| "custom.gguf".to_owned());

    let line = preview_line(&answers(Some(ServiceRole::Local)), Some(&root), &env)
        .expect("an absent model is announced");

    assert!(line.starts_with("custom.gguf (397 MB)"), "{line}");
    assert!(!line.contains(GGUF_FILE_NAME), "{line}");
}
