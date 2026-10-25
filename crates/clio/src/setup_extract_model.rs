// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for putting the extract service's model file on the
//! host before `clio setup` writes anything: it asks the resolved stack which file
//! the container will open and, when that file is the one the project ships and it
//! is absent, downloads it.
//!
//! ## Owns
//! - The `ModelFetcher` seam, its real HTTP implementation, and the download
//!   address and advertised size of the shipped model
//! - The atomic download: a `<dest>.part` file beside the destination, one 64 KiB
//!   chunk at a time, renamed into place only after the byte count matches
//! - The lines a run prints while it fetches: present, downloading, progress, and
//!   downloaded
//! - The cheap presence check the preview announces the download with
//!
//! ## Does not own
//! - The host path the container opens (see `compose_model`)
//! - The compose root, the materialized assets, and the service list (see
//!   `compose_cli`, `compose_assets`, `setup_preview`)
//! - When the fetch runs, or what a failure does to the run (see `setup_cli`)
//!
//! ## Boundary
//!
//! Changes to what is downloaded, or to how a download is written, should normally
//! require changes here. Changes to the host path, the compose root, or the run's
//! order should not.
//!
//! Keep this module focused on the model file's arrival.
//!
//! # Why the fetch is here rather than in the operator's hands
//!
//! The file is not shipped, so someone has to fetch it. Printing a command for the
//! operator to copy asks them to do the tool's work and leaves the run half
//! finished; fetching it here means the files this run writes name a stack whose
//! model is already on disk. The manual command survives only in the failure
//! message, for a machine where the download cannot reach the Hub.

use crate::compose_assets::{self, GGUF_FILE_NAME};
use crate::compose_cli;
use crate::compose_docker::{ComposeTarget, DockerRunner, Stack};
use crate::compose_model;
use crate::setup_preview;
use crate::setup_services::ServiceRole;
use crate::setup_steps::Answers;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Hub address of the shipped model. The repository moved owners, and the Hub
/// redirects the old name, so redirects are left alone on the request.
pub(crate) const GGUF_URL: &str = "https://huggingface.co/DevQuasar-3/numind.NuExtract-tiny-v1.5-GGUF/resolve/main/numind.NuExtract-tiny-v1.5.Q4_K_M.gguf";

/// The size the Hub advertises, in bytes. A display hint for the preview line and
/// the progress denominator; the real check is the size the response reports.
pub(crate) const GGUF_BYTES: u64 = 397_807_776;

/// The name the compose environment may give the extract model instead of the
/// shipped one.
const GGUF_NAME_ENV: &str = "CLIO_EXTRACT_GGUF";

/// The directory under the compose root the extract service mounts.
const MODELS_DIR: &str = "models";

/// The read size for one chunk of the response body.
const CHUNK: usize = 64 * 1024;

/// How many bytes between two progress lines: roughly twenty-five lines for the
/// shipped model.
const PROGRESS_EVERY: u64 = 16 * 1024 * 1024;

/// How long a connection, and then the response headers, may take. The body has no
/// deadline of its own: a slow link is not a failure, and a stall is visible in the
/// progress lines.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What a run did about the extract model file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The run does not need a local extract model.
    NotNeeded,
    /// The file was already on the host; nothing was downloaded.
    Present { path: PathBuf },
    /// The file was downloaded, checked, and renamed into place.
    Fetched { path: PathBuf, bytes: u64 },
}

/// The download seam, shaped like the `DockerRunner` seam: production passes
/// [`RealFetcher`], and a test passes a stub that writes its own bytes, so the
/// suite never opens a socket.
pub(crate) trait ModelFetcher {
    /// Stream `url` into `dest` by way of `<dest>.part`, reporting the running byte
    /// count after each chunk. Returns the number of bytes written.
    ///
    /// # Errors
    /// A refused request, a byte count that does not match what was advertised, or
    /// a filesystem failure. Any partial file is removed before the error returns.
    fn fetch_to(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64, String>;
}

/// The real fetcher: one `ureq` agent per download, over the platform's trust
/// store.
pub(crate) struct RealFetcher;

impl ModelFetcher for RealFetcher {
    fn fetch_to(
        &self,
        url: &str,
        dest: &Path,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            // A non-2xx answer is a fact to report, not a transport failure, so the
            // status is read below rather than thrown here.
            .http_status_as_error(false)
            // The size check compares the bytes on disk with the advertised size,
            // and a compressed body would make `content-length` a length the file
            // never reaches.
            .accept_encoding("identity")
            .timeout_connect(Some(TIMEOUT))
            .timeout_recv_response(Some(TIMEOUT))
            // No deadline for the whole transfer: it is hundreds of megabytes, and a
            // healthy slow download must not be aborted for being slow.
            .timeout_global(None)
            .build()
            .into();
        let response = agent
            .get(url)
            .call()
            .map_err(|e| format!("the request failed: {e}"))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("the address answered status {status}"));
        }
        // The Hub's own length first, then the transport's. Either may be absent on
        // a chunked response, and an absent length is not a failure.
        let expected = ["x-linked-size", "content-length"].iter().find_map(|name| {
            response
                .headers()
                .get(*name)?
                .to_str()
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        });
        let mut body = response.into_body().into_reader();
        stream_to_file(&mut body, dest, expected, progress)
    }
}

/// A downloaded file that has not been renamed into place yet. Dropping it removes
/// it, so every error path between the first byte and the rename leaves no partial
/// file behind. After a successful rename the removal finds nothing and is
/// ignored.
struct PartFile(PathBuf);

impl Drop for PartFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stream `body` into `dest` through `<dest>.part`, then rename it into place.
///
/// The destination's own directory is used for the temporary file, so the rename
/// is within one filesystem and either replaces the file or leaves the old one
/// alone.
fn stream_to_file(
    body: &mut dyn Read,
    dest: &Path,
    expected: Option<u64>,
    progress: &mut dyn FnMut(u64),
) -> Result<u64, String> {
    let part = part_path(dest);
    let _partial = PartFile(part.clone());
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("cannot create {}: {e}", part.display()))?;
    let mut written = 0u64;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = body
            .read(&mut buffer)
            .map_err(|e| format!("the download stopped after {written} bytes: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        written += read as u64;
        progress(written);
    }
    file.sync_all()
        .map_err(|e| format!("cannot flush {}: {e}", part.display()))?;
    drop(file);
    if let Some(expected) = expected
        && written != expected
    {
        return Err(format!(
            "the download ended after {written} of {expected} bytes, so the file was \
             discarded; run `clio setup` again"
        ));
    }
    std::fs::rename(&part, dest).map_err(|e| {
        format!(
            "cannot move the download into place at {}: {e}",
            dest.display()
        )
    })?;
    Ok(written)
}

/// The half-written sibling of `dest`, on the same filesystem so the rename is one
/// filesystem operation.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// What the preview announces a run will download, or `None` when it downloads
/// nothing.
///
/// A presence check and a sentence, nothing else. It answers for a local extract
/// role, and for a file that is not already at `<compose root>/models/<name>`,
/// where the name is the one the compose environment chose or the shipped one.
///
/// It creates no directory, asks Docker nothing, and opens no socket: this runs
/// while a preview is being drawn, before the operator has agreed to anything, so
/// it must not be able to change the machine it reports on. The root is the
/// `--compose-dir` value, or the working directory, and it is joined rather than
/// resolved, for the same reason.
#[must_use]
pub(crate) fn preview_line(
    answers: &Answers,
    dir: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if answers.extract != Some(ServiceRole::Local) {
        return None;
    }
    let name = env(GGUF_NAME_ENV).unwrap_or_else(|| GGUF_FILE_NAME.to_owned());
    let root = dir.map_or_else(|| PathBuf::from("."), PathBuf::from);
    if root.join(MODELS_DIR).join(&name).is_file() {
        return None;
    }
    Some(format!(
        "{name} ({} MB) from the Hugging Face Hub, before it writes the config",
        size_mb(GGUF_BYTES)
    ))
}

/// Put the extract model on the host for this machine, printing what it does.
///
/// # Errors
/// The compose root or its files could not be prepared, the resolved stack could
/// not be read, the stack names a model the project does not ship, or the download
/// failed.
pub(crate) fn ensure_present(
    out: &mut dyn Write,
    answers: &Answers,
    runner: &dyn DockerRunner,
    dir: Option<&str>,
    fetcher: &dyn ModelFetcher,
) -> Result<Outcome, String> {
    let target = ComposeTarget::detect(std::env::consts::OS, std::env::consts::ARCH)?;
    ensure_present_in(out, answers, runner, dir, target, fetcher)
}

/// [`ensure_present`] with the platform injected, so no test depends on the host
/// triple.
///
/// The order is the point: the role decides whether anything is needed, the
/// resolved stack decides where the file goes, and only an absent file the project
/// ships is downloaded.
///
/// # Errors
/// As [`ensure_present`].
pub(crate) fn ensure_present_in(
    out: &mut dyn Write,
    answers: &Answers,
    runner: &dyn DockerRunner,
    dir: Option<&str>,
    target: ComposeTarget,
    fetcher: &dyn ModelFetcher,
) -> Result<Outcome, String> {
    if answers.extract != Some(ServiceRole::Local) {
        return Ok(Outcome::NotNeeded);
    }
    let stack = Stack::from_services(&setup_preview::local_services(answers))?;
    let root = compose_cli::resolve_root(dir)
        .map_err(|e| format!("cannot resolve the compose root: {e}"))?;
    compose_assets::materialize(&root, target)
        .map_err(|e| format!("cannot prepare the compose files: {e}"))?;
    let Some(path) = compose_model::resolved_host_model_path(&root, target, stack, runner)? else {
        return Ok(Outcome::NotNeeded);
    };
    if path.is_file() {
        let _ = writeln!(
            out,
            "  Extract model: {} is already at {}",
            file_name(&path),
            path.display()
        );
        return Ok(Outcome::Present { path });
    }
    // Any other name is a file the project has no source for, and a download would
    // either 404 or fetch a file the container does not open.
    if file_name(&path) != GGUF_FILE_NAME {
        return Err(compose_model::missing_model_error(&path));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let total = size_mb(GGUF_BYTES);
    let _ = writeln!(
        out,
        "  Extract model: downloading {GGUF_FILE_NAME} ({total} MB) from the Hugging Face Hub"
    );
    let mut shown = 0u64;
    let mut announced = false;
    let mut report = |written: u64| {
        if !announced || written - shown >= PROGRESS_EVERY {
            announced = true;
            shown = written;
            let _ = writeln!(
                out,
                "  Extract model: {} MB of {total} MB ({}%)",
                size_mb(written),
                percent(written)
            );
        }
    };
    let bytes = fetcher
        .fetch_to(GGUF_URL, &path, &mut report)
        .map_err(|reason| fetch_error(&path, &reason))?;
    drop(report);
    let _ = writeln!(
        out,
        "  Extract model: downloaded {} MB to {}",
        size_mb(bytes),
        path.display()
    );
    Ok(Outcome::Fetched { path, bytes })
}

/// The last path segment of a model path, or an empty string for a path that has
/// none.
fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

/// The failure an operator sees after a refused or incomplete download: the
/// directory that actually holds the model, and the manual command that fills it.
fn fetch_error(dest: &Path, reason: &str) -> String {
    format!(
        "cannot download the extract model into {}: {reason}; {}",
        destination_directory(dest),
        compose_model::manual_fetch(dest)
    )
}

/// The directory that holds the model. Read from the resolved path, never written
/// out as a relative literal: the compose root can live anywhere, and a literal
/// names a directory under whatever directory the operator happens to be in.
fn destination_directory(dest: &Path) -> String {
    match dest.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.display().to_string(),
        _ => dest.display().to_string(),
    }
}

/// A byte count in millions of bytes, the unit the Hub advertises the size in.
fn size_mb(bytes: u64) -> u64 {
    bytes / 1_000_000
}

/// The share of the advertised size that has arrived, capped so a larger answer
/// than advertised cannot print over 100%.
fn percent(written: u64) -> u64 {
    (written.saturating_mul(100) / GGUF_BYTES).min(100)
}

#[cfg(test)]
#[path = "setup_extract_model_tests.rs"]
pub(crate) mod tests;

/// The real fetcher's transport, driven against a local server rather than the Hub.
#[cfg(test)]
#[path = "setup_extract_model_http_tests.rs"]
pub(crate) mod http_tests;
