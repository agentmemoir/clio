// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the preview's one line about a
//! download: that a local extractor announces it before anything starts, and that
//! a run which downloads nothing says nothing.
//!
//! ## Owns
//! - The `Will fetch` line under test, from the answers to the drawn preview
//!
//! ## Does not own
//! - The presence check itself (see `setup_extract_model_preview_tests`)
//! - The rest of the drawn preview (see `setup_preview_tests`)
//!
//! ## Boundary
//!
//! Changes to what the preview promises about the model download should normally
//! update tests here. Changes to the rest of the layout should not.
//!
//! Keep this module focused on the announcement.

use super::*;

#[test]
fn a_local_extractor_announces_its_download_before_anything_starts() {
    // A local extractor means the model file has to be on the host, and the default
    // compose root in the fixture holds no model, so the download is a thing this run
    // will do.
    let run = Harness::fresh().typing(MANAGED).typing(ALL_LOCAL).run();
    let drawn = run.preview_text();
    let fetch = drawn
        .find("Will fetch")
        .unwrap_or_else(|| panic!("the download must be announced:\n{drawn}"));
    let start = drawn
        .find("Will start")
        .unwrap_or_else(|| panic!("the services are still named:\n{drawn}"));
    assert!(
        fetch < start,
        "the download happens before anything starts, so its line comes first:\n{drawn}"
    );
    assert!(
        drawn.contains("numind.NuExtract-tiny-v1.5.Q4_K_M.gguf (397 MB)"),
        "the line names the file and the size it will cost:\n{drawn}"
    );
}

#[test]
fn a_model_the_compose_root_already_holds_is_not_announced() {
    // The file is where the stack expects it, so this run downloads nothing and has
    // nothing to announce.
    let dir = crate::setup_support_tests::temp_dir("preview-model-present");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models");
    std::fs::write(models.join(crate::compose_assets::GGUF_FILE_NAME), b"stub")
        .expect("write the model");
    let run = Harness::fresh()
        .with_compose_dir(&dir.to_string_lossy())
        .typing(MANAGED)
        .typing(ALL_LOCAL)
        .run();

    assert_eq!(run.answers().extract, Some(ServiceRole::Local));
    assert!(
        !run.preview_text().contains("Will fetch"),
        "an extractor whose model is already there downloads nothing:\n{}",
        run.preview_text()
    );
}

#[test]
fn an_extractor_that_is_not_local_announces_no_download() {
    // Hosted extraction records a skip, so it is answered by flag: a typed line
    // would answer the question the flag already answered. The first run takes the
    // three service defaults, and extraction's default is skip.
    for run in [
        Harness::fresh().typing(MANAGED).typing(SERVICES).run(),
        Harness::fresh()
            .flag("extract", "hosted")
            .typing(MANAGED)
            .typing("\n\n")
            .run(),
    ] {
        assert_eq!(run.answers().extract, Some(ServiceRole::Skip));
        assert!(
            !run.preview_text().contains("Will fetch"),
            "a skipped extractor downloads nothing:\n{}",
            run.preview_text()
        );
    }
}
