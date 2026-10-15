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
//! - The seal gate: an unsealed string at a sealed path is refused
//! - The overwrite guard that refuses to replace a file that no longer loads
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
use std::path::Path;

/// Load a flat `{ "dot.path": value, ... }` or nested JSON object into an overlay.
///
/// Missing files yield an empty overlay (not an error). Every value at a
/// sealed path must be a sealed envelope and is opened, so the overlay a
/// caller receives holds plaintext and the rest of the runtime needs no
/// knowledge of the envelope. An unsealed string at a sealed path is refused:
/// the file was hand-edited or written by something other than the save path,
/// and reading it would accept a plaintext secret. Values at ordinary paths
/// come back unchanged, which is what keeps plain config plain.
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
/// # Errors
/// The existing file cannot be loaded (unsealed secret, bad JSON, unreadable).
pub(crate) fn ensure_overwritable(path: &Path) -> Result<(), AmError> {
    // Only regular files carry prior settings to protect; a directory path
    // falls through to the save, which already fails it as `Internal`.
    if path.is_file() {
        load_overlay_file(path).map(|_| ())?;
    }
    Ok(())
}

fn load_overlay_inner(path: &Path) -> Result<Overlay, AmError> {
    if !path.exists() {
        return Ok(Overlay::new());
    }
    let text = fs::read_to_string(path).map_err(|e| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            format!("failed to read deployment config {}: {e}", path.display()),
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|e| {
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
/// The gate is the path policy, not the value: a value the overlay places at a
/// path [`secret_seal::is_sealed_path`] names must carry the envelope prefix.
/// A failure names the file and the dot-path, and never the value: a
/// half-opened overlay would hand a consumer a sealed string where it expects
/// a secret, and the caller has to stop rather than continue with that.
fn open_sealed(overlay: &mut Overlay, path: &Path) -> Result<(), AmError> {
    let paths: Vec<String> = overlay.keys().cloned().collect();
    for key in paths {
        let Some(stored) = overlay.get(&key).and_then(Value::as_str) else {
            continue;
        };
        if !secret_seal::is_sealed_path(&key) {
            continue;
        }
        if !secret_seal::is_sealed(stored) {
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
