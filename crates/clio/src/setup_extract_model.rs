// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for putting the extract service's model file on the
//! host before `clio setup` writes anything: it asks the resolved stack which file
//! the container will open and, when that file is the one the project ships and it
//! is absent, downloads it or installs a local copy of it.
//!
//! ## Owns
//! - The download address, the published digest, and the advertised size of the
//!   shipped model
//! - The environment values that replace the download with a mirror or with a
//!   local copy of the file
//! - The lines a run prints while it fetches: present, downloading, progress, and
//!   downloaded
//! - The preview's announcement: the fetch it will make, or the reason the run
//!   will stop before it writes the config
//!
//! ## Does not own
//! - The host path the container opens (see `compose_model`)
//! - The embedded asset set, the backup guard, and the bind-mount check (see
//!   `compose_assets`), which this module asks for the compose files it needs
//!   before it resolves a model path
//! - The service list (see `setup_preview`), the root resolution (see
//!   `compose_cli`), and when the fetch runs (see `setup_cli`)
//! - The transfer itself, its length and digest checks, and the rename (see
//!   `setup_extract_model_download`)
//!
//! ## Boundary
//!
//! Changes to what is downloaded, or to when a run refuses to fetch, should
//! normally require changes here. Changes to the host path, the compose root, or
//! the run's order should not.
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

use crate::compose_assets::{self, GGUF_FILE_NAME, GGUF_REPO, GGUF_SHA256};
use crate::compose_cli;
use crate::compose_docker::{ComposeTarget, DockerRunner, Stack};
use crate::compose_model;
use crate::setup_preview;
use crate::setup_services::ServiceRole;
use crate::setup_steps::Answers;
use std::io::Write;
use std::path::{Path, PathBuf};

#[path = "setup_extract_model_download.rs"]
mod download;

pub(crate) use download::{ModelFetcher, RealFetcher, install_local};

/// The size the Hub advertises, in bytes. A display hint for the preview line and
/// the progress denominator; the real check is the size the response reports.
pub(crate) const GGUF_BYTES: u64 = 397_807_776;

/// The name the compose environment may give the extract model instead of the
/// shipped one.
const GGUF_NAME_ENV: &str = "CLIO_EXTRACT_GGUF";

/// A mirror to download the model from instead of the Hub.
const URL_ENV: &str = "CLIO_EXTRACT_MODEL_URL";

/// A local copy of the model to install instead of downloading one.
const FILE_ENV: &str = "CLIO_EXTRACT_MODEL_FILE";

/// The directory under the compose root the extract service mounts.
const MODELS_DIR: &str = "models";

/// How many bytes between two progress lines: roughly twenty-five lines for the
/// shipped model.
const PROGRESS_EVERY: u64 = 16 * 1024 * 1024;

/// The Hub address of the shipped model, derived from the one repository id and
/// the one file name the rest of the project prints.
#[must_use]
pub(crate) fn download_url() -> String {
    format!("https://huggingface.co/{GGUF_REPO}/resolve/main/{GGUF_FILE_NAME}")
}

/// What the preview says about the model: the download the run will make, and,
/// when the run will refuse instead, the reason it will print.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Announced {
    /// What `Will fetch` says, or `None` when the run downloads nothing.
    pub(crate) will_fetch: Option<String>,
    /// What `Will stop` says, or `None` when the run goes on to write the config.
    pub(crate) will_stop: Option<String>,
}

/// What the preview announces a run will do about the model file.
///
/// A presence check and a sentence, nothing else. It answers for a local extract
/// role: a file that is not already at `<compose root>/models/<name>` is announced
/// as a download only when it is the file the project ships, because any other
/// name is one the run refuses to fetch.
///
/// It creates no directory, asks Docker nothing, and opens no socket: this runs
/// while a preview is being drawn, before the operator has agreed to anything, so
/// it must not be able to change the machine it reports on. The root is resolved
/// the way the download resolves it — the `--compose-dir` value, or the working
/// directory — and joined rather than created, for the same reason.
#[must_use]
pub(crate) fn announcement(
    answers: &Answers,
    dir: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Announced {
    if answers.extract != Some(ServiceRole::Local) {
        return Announced::default();
    }
    let Some(root) = preview_root(dir) else {
        return Announced::default();
    };
    let name = env(GGUF_NAME_ENV).unwrap_or_else(|| GGUF_FILE_NAME.to_owned());
    let path = root.join(MODELS_DIR).join(&name);
    if path.is_file() {
        return Announced::default();
    }
    if outside_models(&path) {
        return Announced {
            will_fetch: None,
            will_stop: Some(format!(
                "before it writes the config: the stack names an extract model outside {MODELS_DIR}/, \
                 at {}, so nothing would be downloaded",
                path.display()
            )),
        };
    }
    if name != GGUF_FILE_NAME {
        return Announced {
            will_fetch: None,
            will_stop: Some(format!(
                "before it writes the config: {}",
                compose_model::missing_model_error(&path)
            )),
        };
    }
    Announced {
        will_fetch: Some(format!(
            "{name} ({} MB) from the Hugging Face Hub, before it writes the config",
            size_mb(GGUF_BYTES)
        )),
        will_stop: None,
    }
}

/// The root the preview joins: the `--compose-dir` value, or the working
/// directory the download resolves its own root from.
///
/// Not `compose_cli::resolve_root`, which creates the directory: a preview must
/// not make the machine it reports on.
fn preview_root(dir: Option<&str>) -> Option<PathBuf> {
    match dir {
        Some(dir) => Some(PathBuf::from(dir)),
        None => std::env::current_dir().ok(),
    }
}

/// Put the extract model on the host for this machine, printing what it does.
///
/// # Errors
/// The compose root or its files could not be prepared, the resolved stack could
/// not be read, the stack names a model the project does not ship or one outside
/// `models/`, or the download failed.
pub(crate) fn ensure_present(
    out: &mut dyn Write,
    answers: &Answers,
    runner: &dyn DockerRunner,
    dir: Option<&str>,
    fetcher: &dyn ModelFetcher,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let target = ComposeTarget::detect(std::env::consts::OS, std::env::consts::ARCH)?;
    ensure_present_in(out, answers, runner, dir, target, fetcher, env)
}

/// [`ensure_present`] with the platform injected, so no test depends on the host
/// triple.
///
/// The order is the point: the role decides whether anything is needed, the
/// resolved stack decides where the file goes, and only an absent file the project
/// ships is fetched. The two environment values are read here rather than earlier
/// because each of them replaces the transfer for a missing file, and a present
/// file needs neither.
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
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    if answers.extract != Some(ServiceRole::Local) {
        return Ok(());
    }
    let stack = Stack::from_services(&setup_preview::local_services(answers))?;
    let root = compose_cli::resolve_root(dir)
        .map_err(|e| format!("cannot resolve the compose root: {e}"))?;
    // The compose files the query below reads have to exist before Compose is
    // asked, so this asks `compose_assets` for them; the asset set is still that
    // module's to own.
    compose_assets::materialize(&root, target)
        .map_err(|e| format!("cannot prepare the compose files: {e}"))?;
    let Some(path) = compose_model::resolved_host_model_path(&root, target, stack, runner)? else {
        return Ok(());
    };
    if path.is_file() {
        let _ = writeln!(
            out,
            "  Extract model: {} is already at {}",
            file_name(&path),
            path.display()
        );
        return Ok(());
    }
    // A name from the environment is joined onto `models/`, so one carrying its own
    // parent segments would put the file anywhere on the host.
    if outside_models(&path) {
        return Err(format!(
            "the stack names an extract model outside {MODELS_DIR}/, at {}; a model name must be a \
             file in {MODELS_DIR}/, not a path",
            path.display()
        ));
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
    // The two ways a missing file arrives. The local copy replaces the download
    // entirely: an air-gapped or metered machine has no socket to open, so neither
    // the fetcher nor the address is touched.
    let local_copy = env(FILE_ENV);
    if let Some(source) = &local_copy {
        let _ = writeln!(
            out,
            "  Extract model: installing {GGUF_FILE_NAME} from {source}"
        );
    } else {
        let _ = writeln!(
            out,
            "  Extract model: downloading {GGUF_FILE_NAME} ({total} MB) from the Hugging Face Hub"
        );
    }
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
    let (bytes, verb) = if let Some(source) = &local_copy {
        let bytes = install_local(Path::new(source), &path, GGUF_SHA256, &mut report)
            .map_err(|reason| fetch_error(&path, &reason))?;
        (bytes, "installed")
    } else {
        let url = env(URL_ENV).unwrap_or_else(download_url);
        let bytes = fetcher
            .fetch_to(&url, &path, GGUF_SHA256, &mut report)
            .map_err(|reason| fetch_error(&path, &reason))?;
        (bytes, "downloaded")
    };
    drop(report);
    // A transfer that wrote nothing is refused inside the streaming helper, so a
    // zero byte count cannot reach this line.
    let _ = writeln!(
        out,
        "  Extract model: {verb} {} MB to {}",
        size_mb(bytes),
        path.display()
    );
    Ok(())
}

/// The last path segment of a model path, or an empty string for a path that has
/// none.
fn file_name(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

/// Whether a model path leaves the `<root>/models` directory the compose file
/// mounts. Read from the path's own parent rather than from the name, so a name
/// carrying `..` or an absolute path is caught wherever it came from.
fn outside_models(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_none_or(|dir| dir != MODELS_DIR)
}

/// The failure an operator sees after a refused or incomplete download: the
/// directory that actually holds the model, every way forward, and the manual
/// command that fills it.
fn fetch_error(dest: &Path, reason: &str) -> String {
    format!(
        "cannot download the extract model into {}: {reason}; check the network and rerun, point \
         `{URL_ENV}` at a mirror, set `{FILE_ENV}` to a local copy of the same file, or choose \
         `--extract skip`. {}",
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
