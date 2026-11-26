// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading a deployment overlay file and opening
//! the values it stores sealed.
//!
//! ## Owns
//! - Reading flat or nested JSON into an overlay, with missing files empty
//! - The seal gate: an envelope opens at any path, and an unsealed string at a
//!   sealed path is refused
//! - The overwrite guard that reads the file once, holds it open, refuses a
//!   file that no longer loads, and refuses to replace a different entry or a
//!   symbolic link at the path
//!
//! ## Does not own
//! - Sealing values on the way out (see `save`)
//! - Where the file lives (see `path`)
//! - The envelope format and sealed-path list (see `crate::secret_seal`)
//!
//! ## Boundary
//!
//! Changes to how the overlay is read or how the gate refuses belong here.
//! Changes to the save side should not require changes here.
//!
//! Keep this module focused on loading and opening the deployment overlay.

use crate::config::merge::{Overlay, flatten_object};
use crate::error::{AmError, ErrorCode};
use crate::secret_seal;
use serde_json::Value;
use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Load a flat `{ "dot.path": value, ... }` or nested JSON object into an overlay.
///
/// Missing files yield an empty overlay (not an error). Every sealed envelope is
/// opened wherever it sits, so the overlay a caller receives holds plaintext and
/// the rest of the runtime needs no knowledge of the envelope. An unsealed string
/// at a sealed path is refused: the file was hand-edited or written by something
/// other than the save path, and reading it would accept a plaintext secret.
/// Values at ordinary paths come back unchanged, which is what keeps plain config
/// plain.
///
/// # Errors
/// I/O or JSON parse failures, an unsealed value at a sealed path, or a sealed
/// value that cannot be opened.
pub fn load_overlay_file(path: &Path) -> Result<Overlay, AmError> {
    load_overlay_inner(path).inspect_err(|err| {
        crate::log::warn(
            "deployment overlay not loaded",
            &[
                ("path", &path.to_string_lossy()),
                ("error", err.message.as_str()),
            ],
        );
    })
}

/// Refuse to overwrite a deployment file that no longer loads, so a refused
/// legacy file is never silently replaced by an in-memory overlay that
/// contains only the newest set. Setup backs the file up before replacing it,
/// so it keeps calling `save_overlay_file` directly; this gate is only for
/// `config_set`, which has no backup.
///
/// The file is opened once and judged from the bytes read through that open
/// handle. The handle stays in the returned guard, so the save that follows
/// can refuse to replace a path that no longer names the file that was
/// judged.
///
/// # Errors
/// The existing file cannot be loaded (unsealed secret, bad JSON, unreadable).
pub(crate) fn check_overwritable(path: &Path) -> Result<CheckedOverwrite, AmError> {
    // Only regular files carry prior settings to protect; a directory path
    // falls through to the save, which already fails it as `Internal`.
    if !path.is_file() {
        return Ok(CheckedOverwrite {
            _file: None,
            identity: None,
        });
    }
    let mut file = fs::File::open(path).map_err(|e| read_failure(path, &e))?;
    let meta = file.metadata().map_err(|e| read_failure(path, &e))?;
    let text = read_text(&mut file, path)?;
    parse_overlay(&text, path)?;
    Ok(CheckedOverwrite {
        _file: Some(file),
        identity: Some(FileIdentity::from_metadata(&meta)),
    })
}

/// A deployment file that passed the overwrite check.
///
/// The open handle keeps the judged file alive. `ensure_unchanged` compares
/// the path with that file just before a rename replaces it.
pub(crate) struct CheckedOverwrite {
    /// Keeps the judged file open for the life of the guard.
    _file: Option<fs::File>,
    /// `None` means no file sat at the path when the check ran.
    identity: Option<FileIdentity>,
}

impl CheckedOverwrite {
    /// Refuse when the path no longer names the file this check judged.
    ///
    /// The check looks at the entry itself, not what it points at, because the
    /// rename replaces the entry. A symbolic link at the path is refused: the
    /// loader follows it but the rename would replace the link entry, so the
    /// two steps would judge different objects.
    ///
    /// # Errors
    /// The entry is a symbolic link, the path is gone, is no longer a regular
    /// file, names a different file, or gained a regular file after an absent
    /// check.
    pub(crate) fn ensure_unchanged(&self, path: &Path) -> Result<(), AmError> {
        let entry = fs::symlink_metadata(path).ok();
        if entry
            .as_ref()
            .is_some_and(|meta| meta.file_type().is_symlink())
        {
            return Err(symlink_failure(path));
        }
        match self.identity {
            None => {
                if entry.is_some_and(|meta| meta.is_file()) {
                    return Err(changed_failure(path));
                }
            }
            Some(expected) => {
                let same = entry.is_some_and(|meta| meta.is_file() && expected.matches(&meta));
                if !same {
                    return Err(changed_failure(path));
                }
            }
        }
        Ok(())
    }
}

/// The identity of one file, so a later stat can tell the same file apart from
/// a different entry at the same path.
#[derive(Debug, Clone, Copy)]
struct FileIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    fn from_metadata(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }

    /// Other platforms carry no comparison fields, so they cannot tell two
    /// entries apart. The path-must-still-be-a-file part of the check still
    /// runs there.
    #[cfg(not(unix))]
    fn from_metadata(_meta: &fs::Metadata) -> Self {
        Self {}
    }

    #[cfg(unix)]
    fn matches(self, meta: &fs::Metadata) -> bool {
        self.dev == meta.dev() && self.ino == meta.ino()
    }

    #[cfg(not(unix))]
    fn matches(self, _meta: &fs::Metadata) -> bool {
        true
    }
}

/// The refusal for a deployment path that is a symbolic link.
fn symlink_failure(path: &Path) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "deployment config {} is a symbolic link, and Clio will not replace a link entry. \
             Replace the link with the real file and run the command again",
            path.display()
        ),
    )
}

/// The refusal for a deployment file that changed after it was checked.
fn changed_failure(path: &Path) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "deployment config {} changed while Clio checked it, so Clio refused to replace it \
             and wrote nothing. Run the command again",
            path.display()
        ),
    )
}

fn load_overlay_inner(path: &Path) -> Result<Overlay, AmError> {
    if !path.exists() {
        return Ok(Overlay::new());
    }
    let mut file = fs::File::open(path).map_err(|e| read_failure(path, &e))?;
    let text = read_text(&mut file, path)?;
    parse_overlay(&text, path)
}

/// Read the whole file through `source`.
fn read_text(mut source: impl Read, path: &Path) -> Result<String, AmError> {
    let mut text = String::new();
    source
        .read_to_string(&mut text)
        .map_err(|e| read_failure(path, &e))?;
    Ok(text)
}

fn read_failure(path: &Path, error: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "failed to read deployment config {}: {error}",
            path.display()
        ),
    )
}

/// Parse and open the text of one deployment file. Shared by the plain load
/// and the overwrite check, so both judge the same bytes.
fn parse_overlay(text: &str, path: &Path) -> Result<Overlay, AmError> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!("deployment config parse error: {e}"),
        )
    })?;
    let mut out = Overlay::new();
    match value {
        // Always flatten: a file may mix dotted keys with nested objects, and
        // the sealed gate runs on the flattened dot-paths, so nesting hides
        // nothing. Follows the file-overlay pattern in `super` (`load_file_json`).
        Value::Object(map) => flatten_object("", &map, &mut out),
        _ => {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "deployment config root must be a JSON object",
            ));
        }
    }
    open_sealed(&mut out, path)?;
    Ok(out)
}

/// Open every sealed value in a freshly loaded overlay, and refuse an
/// unsealed string at a sealed path.
///
/// Two rules, one gate. An envelope opens wherever it sits, so a value the save
/// sealed because of what it holds — a URL with userinfo credentials — comes back
/// readable at the path it was written to. A stored string that is not an
/// envelope is refused when [`secret_seal::is_sealed_path`] names its path, and
/// otherwise left alone as plain config. A failure names the file and the
/// dot-path, and never the value: a half-opened overlay would hand a consumer a
/// sealed string where it expects a secret, and the caller has to stop rather
/// than continue with that.
fn open_sealed(overlay: &mut Overlay, path: &Path) -> Result<(), AmError> {
    let paths: Vec<String> = overlay.keys().cloned().collect();
    for key in paths {
        let Some(stored) = overlay.get(&key).and_then(Value::as_str) else {
            continue;
        };
        if !secret_seal::is_sealed(stored) {
            if !secret_seal::is_sealed_path(&key) {
                continue;
            }
            return Err(unsealed_failure(&key, path));
        }
        let opened = secret_seal::open(stored).map_err(|err| {
            AmError::new(
                err.code,
                format!(
                    "deployment config {}: cannot read the sealed value at `{key}`: {}. Clio did \
                     not rewrite the file; restore the Clio encryption key this value was sealed \
                     under, or set the value again through the tooling so the next save seals it",
                    path.display(),
                    err.message
                ),
            )
        })?;
        overlay.insert(key, Value::String(opened));
    }
    Ok(())
}

/// The refusal for an unsealed value at a path Clio stores sealed.
///
/// The message names the file and the dot-path, echoes nothing about the
/// value, and points at re-sealing through the existing save path — never at
/// editing the file's plaintext by hand, which would leave the secret readable
/// on disk.
fn unsealed_failure(key: &str, path: &Path) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "deployment config {}: the value at `{key}` is not sealed, and `{key}` is a path Clio \
                 stores sealed. Clio refused the file rather than read a plaintext secret. Set the \
                 value again through the tooling (`clio setup`, or `clio config set {key} --scope \
                 deployment`) so the save seals it",
            path.display(),
        ),
    )
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod load_tests;
