// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the preview's announcement about
//! the model: the download it promises, the run it says will stop instead, and the
//! cases where it says nothing.
//!
//! ## Owns
//! - `announcement` behavior under test for `setup_extract_model`
//!
//! ## Does not own
//! - The download itself, and its progress lines (see `setup_extract_model_tests`)
//! - The rendered preview around the line (see `setup_preview_tests_fetch_tests`)
//!
//! ## Boundary
//!
//! Changes to what the preview promises about the download should normally update
//! tests here. Changes to what a download writes should not.
//!
//! Keep this module focused on the promise, not on the transfer.

use super::*;

/// The line a run announces for a shipped model that is absent.
#[test]
fn an_absent_shipped_model_is_announced_as_a_download() {
    let dir = temp_dir("preview-absent");
    let root = dir.to_string_lossy().into_owned();

    let announced = announcement(&answers(Some(ServiceRole::Local)), Some(&root), &no_env);

    let line = announced
        .will_fetch
        .expect("an absent shipped model is announced");
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
    assert_eq!(
        announced.will_stop, None,
        "a run that will fetch has no reason to say it stops"
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

    assert_eq!(
        announcement(&answers(Some(ServiceRole::Local)), Some(&root), &no_env),
        Announced::default(),
        "there is nothing to announce"
    );
}

/// A name the code will not fetch must not be announced as a fetch: the preview
/// says the run stops before it writes the config, and why.
#[test]
fn a_name_the_project_does_not_ship_is_announced_as_a_stop() {
    let dir = temp_dir("preview-named");
    let root = dir.to_string_lossy().into_owned();
    let env = |key: &str| (key == "CLIO_EXTRACT_GGUF").then(|| "custom.gguf".to_owned());

    let announced = announcement(&answers(Some(ServiceRole::Local)), Some(&root), &env);

    assert_eq!(
        announced.will_fetch, None,
        "a name with no source is never announced as a download"
    );
    let notice = announced.will_stop.expect("the run stops instead");
    assert!(notice.contains("before it writes the config"), "{notice}");
    assert!(notice.contains("custom.gguf"), "{notice}");
    assert!(
        notice.contains(GGUF_FILE_NAME),
        "the notice names the model the project ships instead: {notice}"
    );
}

/// The shipped name, one directory up: the run refuses, so the preview says so
/// rather than promising a download outside `models/`.
#[test]
fn a_name_that_leaves_the_models_directory_is_announced_as_a_stop() {
    let dir = temp_dir("preview-escapes");
    let root = dir.to_string_lossy().into_owned();
    let escaped = format!("../{GGUF_FILE_NAME}");
    let env = move |key: &str| (key == "CLIO_EXTRACT_GGUF").then(|| escaped.clone());

    let announced = announcement(&answers(Some(ServiceRole::Local)), Some(&root), &env);

    assert_eq!(announced.will_fetch, None, "nothing is downloaded");
    let notice = announced.will_stop.expect("the run stops instead");
    assert!(notice.contains("outside models/"), "{notice}");
}

#[test]
fn an_extract_role_that_is_not_local_announces_nothing() {
    let dir = temp_dir("preview-not-local");
    let root = dir.to_string_lossy().into_owned();

    for role in [ServiceRole::Skip, ServiceRole::Hosted] {
        assert_eq!(
            announcement(&answers(Some(role)), Some(&root), &no_env),
            Announced::default(),
            "a {role:?} extract downloads nothing and stops for nothing"
        );
    }
}

#[test]
fn an_extract_role_nobody_chose_announces_nothing() {
    // A run that never reached the question has no extract role at all, and a line
    // promising a download it cannot describe would be a guess.
    let dir = temp_dir("preview-unanswered");
    let root = dir.to_string_lossy().into_owned();

    assert_eq!(
        announcement(&answers(None), Some(&root), &no_env),
        Announced::default(),
        "no answer means no announcement"
    );
}
