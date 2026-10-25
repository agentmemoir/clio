// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the extract model's arrival: whether a run needs the file, where the
//! resolved stack says it goes, what a download writes, and what a failure leaves
//! behind. The Hub is never reached: the fetcher is a stub that writes its own
//! bytes, and the streaming path is driven with an in-memory reader.
//!
//! ## Owns
//! - Download, presence, refusal, and environment overrides under test for
//!   `setup_extract_model`
//!
//! ## Does not own
//! - The streaming write, the length and digest checks, and the rename (see
//!   `setup_extract_model_download`)
//! - The host-path derivation from a resolved stack (see `compose_model_tests`)
//! - The run order around the fetch (see `setup_cli_fail_tests`)
//!
//! ## Boundary
//!
//! Changes to what a fetch prints or writes should normally update tests here.
//! Changes to what Compose is asked should not.

use super::*;
use crate::compose_docker::tests::{RecordingRunner, extract_config, extract_config_with};
use crate::setup_services::ServiceRole;
use crate::setup_steps::Answers;
use crate::setup_support_tests::temp_dir;
use std::cell::RefCell;

/// An environment that sets nothing, so the shipped model and the Hub are meant.
fn no_env(_key: &str) -> Option<String> {
    None
}

/// A `ModelFetcher` stub: records the address and destination of every download
/// and either writes filler bytes or refuses.
///
/// It never opens a socket, so a suite that drives the fetch runs on a machine with
/// no network at all. The bytes are written to the destination directly rather than
/// through a temporary file: the atomic rename is the real fetcher's, and its
/// failure paths are covered where it is implemented.
pub(crate) struct StubFetcher {
    bytes: u64,
    failure: Option<String>,
    calls: RefCell<Vec<(String, PathBuf)>>,
}

impl StubFetcher {
    /// A fetcher that writes `bytes` filler bytes and answers with the count.
    pub(crate) fn writing(bytes: u64) -> Self {
        Self {
            bytes,
            failure: None,
            calls: RefCell::new(Vec::new()),
        }
    }

    /// A fetcher that records the attempt and refuses with `reason`.
    pub(crate) fn failing(reason: &str) -> Self {
        Self {
            bytes: 0,
            failure: Some(reason.to_owned()),
            calls: RefCell::new(Vec::new()),
        }
    }

    /// One `(url, destination)` pair per attempt, in order.
    pub(crate) fn calls(&self) -> Vec<(String, PathBuf)> {
        self.calls.borrow().clone()
    }
}

impl crate::setup_extract_model::ModelFetcher for StubFetcher {
    fn fetch_to(
        &self,
        url: &str,
        dest: &Path,
        _sha256: &str,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64, String> {
        self.calls
            .borrow_mut()
            .push((url.to_owned(), dest.to_path_buf()));
        if let Some(reason) = &self.failure {
            return Err(reason.clone());
        }
        let mut file = std::fs::File::create(dest)
            .map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
        let filler = vec![b'x'; 64 * 1024];
        let mut written = 0u64;
        while written < self.bytes {
            let chunk = usize::try_from((self.bytes - written).min(filler.len() as u64))
                .expect("a chunk fits the buffer");
            file.write_all(&filler[..chunk])
                .map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
            written += chunk as u64;
            progress(written);
        }
        Ok(written)
    }
}

/// A target that is not this machine: the fetch takes its platform as an
/// argument, so no test depends on the host triple.
const TARGET: ComposeTarget = ComposeTarget::MacArm64;

/// The answers one fetch sees: no database and no other sidecar, so the only
/// service in play is the one the test chooses.
fn answers(extract: Option<ServiceRole>) -> Answers {
    Answers {
        key_source: None,
        sync_key_source: None,
        sync_in_play: false,
        database: None,
        embed_role: None,
        embed: None,
        rerank_role: None,
        rerank: None,
        extract,
    }
}

/// The temporary file this process writes, named the same way the transfer names
/// it.
fn part_of(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.part", std::process::id()));
    dest.with_file_name(name)
}

#[test]
fn an_extract_role_that_is_not_local_needs_no_model() {
    let dir = temp_dir("fetch-not-local");
    let root = dir.to_string_lossy().into_owned();
    let runner = RecordingRunner::succeeding();
    let fetcher = StubFetcher::writing(1_024);
    let mut out = Vec::new();

    ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Skip)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &no_env,
    )
    .expect("a skipped extract needs nothing");

    assert!(
        fetcher.calls().is_empty(),
        "no download may be attempted: {:?}",
        fetcher.calls()
    );
    assert!(
        runner.recorded().is_empty(),
        "nothing may be asked of Compose when no model is needed: {:?}",
        runner.recorded()
    );
    assert!(
        out.is_empty(),
        "a run with nothing to fetch says nothing: {}",
        String::from_utf8_lossy(&out)
    );
}

#[test]
fn a_model_already_on_the_host_is_reported_and_not_fetched() {
    let dir = temp_dir("fetch-present");
    let root = dir.to_string_lossy().into_owned();
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models");
    std::fs::write(models.join(GGUF_FILE_NAME), b"stub").expect("write the model");
    let runner = RecordingRunner::succeeding().with_config(extract_config(&dir));
    let fetcher = StubFetcher::writing(1_024);
    let mut out = Vec::new();

    ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &no_env,
    )
    .expect("a present model needs nothing");

    assert!(fetcher.calls().is_empty(), "nothing may be downloaded");
    let text = String::from_utf8(out).expect("utf8");
    assert!(text.contains("is already at"), "{text}");
    assert!(!text.contains("downloading"), "{text}");
}

#[test]
fn a_missing_shipped_model_is_downloaded_into_the_compose_root() {
    let dir = temp_dir("fetch-absent");
    let root = dir.to_string_lossy().into_owned();
    let runner = RecordingRunner::succeeding().with_config(extract_config(&dir));
    // More than one chunk and more than a megabyte, so the streaming path is
    // exercised rather than one read and the byte count is not rounded to zero.
    let fetcher = StubFetcher::writing(1_200_000);
    let mut out = Vec::new();

    ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &no_env,
    )
    .expect("an absent shipped model is fetched");

    let dest = dir.join("models").join(GGUF_FILE_NAME);
    assert_eq!(
        fetcher.calls(),
        vec![(download_url(), dest.clone())],
        "the Hub address and the compose-resolved destination"
    );
    let written = std::fs::metadata(&dest).expect("the model is on disk");
    assert_eq!(written.len(), 1_200_000, "the file holds the bytes written");
    let text = String::from_utf8(out).expect("utf8");
    assert!(
        text.contains("downloading numind.NuExtract-tiny-v1.5.Q4_K_M.gguf (397 MB)"),
        "{text}"
    );
    assert!(
        text.contains(" MB of 397 MB ("),
        "a progress line carries the bytes of the total and a percent: {text}"
    );
    assert!(
        text.contains(&format!("downloaded 1 MB to {}", dest.display())),
        "the closing line names where the file landed: {text}"
    );
    assert!(
        !text.contains('\r'),
        "one line per event, never a carriage return: {text:?}"
    );
}

#[test]
fn a_model_name_the_project_does_not_ship_is_refused_without_a_download() {
    let dir = temp_dir("fetch-custom");
    let root = dir.to_string_lossy().into_owned();
    let runner =
        RecordingRunner::succeeding().with_config(extract_config_with(&dir, "custom.gguf"));
    let fetcher = StubFetcher::writing(1_024);
    let mut out = Vec::new();

    let err = ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &no_env,
    )
    .expect_err("a name the project does not ship must fail the run");

    assert!(err.contains(GGUF_FILE_NAME), "{err}");
    assert!(err.contains("custom.gguf"), "{err}");
    assert!(
        fetcher.calls().is_empty(),
        "only the shipped file has a source: {:?}",
        fetcher.calls()
    );
}

#[test]
fn a_model_name_that_leaves_the_models_directory_is_refused() {
    let dir = temp_dir("fetch-escapes");
    let root = dir.to_string_lossy().into_owned();
    // The shipped name, one directory up: the shipped-name check alone would pass
    // this and download outside `models/`.
    let escaped = format!("../{GGUF_FILE_NAME}");
    let runner = RecordingRunner::succeeding().with_config(extract_config_with(&dir, &escaped));
    let fetcher = StubFetcher::writing(1_024);
    let mut out = Vec::new();

    let err = ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &no_env,
    )
    .expect_err("a model outside models/ must fail the run");

    assert!(err.contains("outside models/"), "{err}");
    assert!(
        fetcher.calls().is_empty(),
        "nothing may be downloaded outside models/: {:?}",
        fetcher.calls()
    );
    assert!(
        !dir.join(GGUF_FILE_NAME).exists(),
        "nothing may be written outside models/"
    );
}

#[test]
fn a_refused_download_keeps_the_manual_command_and_names_the_directory() {
    let dir = temp_dir("fetch-refused");
    let root = dir.to_string_lossy().into_owned();
    let models = dir.join("models");
    let runner = RecordingRunner::succeeding().with_config(extract_config(&dir));
    let fetcher = StubFetcher::failing("the address answered status 503");
    let mut out = Vec::new();

    let err = ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &no_env,
    )
    .expect_err("a refused download must fail the run");

    assert!(
        err.contains("cannot download the extract model into"),
        "{err}"
    );
    assert!(err.contains("the address answered status 503"), "{err}");
    assert!(
        err.contains("hf download"),
        "the manual command survives for a machine that cannot download: {err}"
    );
    assert!(
        err.contains(&models.display().to_string()),
        "the failure names the directory that actually holds the model: {err}"
    );
    assert!(
        !err.contains("./models"),
        "a relative literal names a directory under the caller's own path, not the stack's \
         root: {err}"
    );
    assert!(
        !part_of(&models.join(GGUF_FILE_NAME)).exists(),
        "no partial file may survive a refused download"
    );
}

/// The failure names every way forward in one sentence, so a machine that cannot
/// download is not left with the manual command as its only exit.
#[test]
fn a_refused_download_names_every_way_forward() {
    let dir = temp_dir("fetch-ways-forward");
    let root = dir.to_string_lossy().into_owned();
    let runner = RecordingRunner::succeeding().with_config(extract_config(&dir));
    let fetcher = StubFetcher::failing("the request failed: no route to host");
    let mut out = Vec::new();

    let err = ensure_present_in(
        &mut out,
        &answers(Some(ServiceRole::Local)),
        &runner,
        Some(&root),
        TARGET,
        &fetcher,
        &no_env,
    )
    .expect_err("a refused download must fail the run");

    assert!(
        err.contains("check the network and rerun"),
        "the network is the first thing to check: {err}"
    );
    assert!(err.contains("`CLIO_EXTRACT_MODEL_FILE`"), "{err}");
    assert!(err.contains("`CLIO_EXTRACT_MODEL_URL`"), "{err}");
    assert!(err.contains("`--extract skip`"), "{err}");
}

#[cfg(test)]
#[path = "setup_extract_model_env_tests.rs"]
mod env_tests;

#[cfg(test)]
#[path = "setup_extract_model_preview_tests.rs"]
mod preview_tests;
