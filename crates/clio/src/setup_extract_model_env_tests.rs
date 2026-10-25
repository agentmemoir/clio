// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the two environment values that
//! replace the download: a mirror address, and a local copy of the file. Neither
//! test opens a socket.
//!
//! ## Owns
//! - `CLIO_EXTRACT_MODEL_URL` and `CLIO_EXTRACT_MODEL_FILE` under test
//!
//! ## Does not own
//! - The presence check and the refusal rules (see `setup_extract_model_tests`)
//! - The transfer and its checks (see `setup_extract_model_download`)
//!
//! ## Boundary
//!
//! Changes to the way a machine without the Hub completes the step should normally
//! update tests here. Changes to what a download writes should not.

use super::*;

#[test]
fn a_mirror_from_the_environment_is_the_address_downloaded() {
    let dir = temp_dir("fetch-mirror");
    let root = dir.to_string_lossy().into_owned();
    let runner = RecordingRunner::succeeding().with_config(extract_config(&dir));
    let fetcher = StubFetcher::writing(1_200_000);
    let env = |key: &str| {
        (key == "CLIO_EXTRACT_MODEL_URL").then(|| "https://mirror.example/m.gguf".to_owned())
    };
    let mut out = Vec::new();

    ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &env,
    )
    .expect("a mirror is a download");

    assert_eq!(
        fetcher.calls().first().map(|(url, _)| url.clone()),
        Some("https://mirror.example/m.gguf".to_owned()),
        "the mirror replaces the Hub address: {:?}",
        fetcher.calls()
    );
}

/// A local copy is installed instead of downloaded, and it is checked the same
/// way: a file whose bytes are not the published ones is refused, with no socket
/// opened and nothing left in the directory.
#[test]
fn a_local_file_from_the_environment_is_checked_and_no_socket_is_opened() {
    let dir = temp_dir("fetch-local-file");
    let root = dir.to_string_lossy().into_owned();
    let runner = RecordingRunner::succeeding().with_config(extract_config(&dir));
    let source = dir.join("elsewhere.gguf");
    std::fs::write(&source, b"not the published bytes").expect("write the local copy");
    let source_text = source.to_string_lossy().into_owned();
    let env = move |key: &str| (key == "CLIO_EXTRACT_MODEL_FILE").then(|| source_text.clone());
    let fetcher = StubFetcher::writing(1_024);
    let mut out = Vec::new();

    let err = ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &env,
    )
    .expect_err("a local copy that does not match the digest must fail the run");

    assert!(
        err.contains("published checksum"),
        "the local copy is checked like a download: {err}"
    );
    assert!(
        fetcher.calls().is_empty(),
        "a local copy replaces the download: {:?}",
        fetcher.calls()
    );
    let dest = dir.join("models").join(GGUF_FILE_NAME);
    assert!(!dest.exists(), "a refused install leaves no model");
    assert!(
        !part_of(&dest).exists(),
        "a refused install leaves no temporary file"
    );
    let text = String::from_utf8(out).expect("utf8");
    assert!(text.contains("installing"), "{text}");
}
