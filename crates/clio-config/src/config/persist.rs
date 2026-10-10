// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for durable deployment-overlay load, backup, and
//! save, and for the owner-only permission rule those saves rely on.
//!
//! ## Owns
//! - Reading/writing flat JSON overlay files for `scope=deployment`
//! - Sealing a secret value on the way out and opening it on the way in
//! - Resolving the deployment config path from an explicit override, the
//!   `CLIO_DEPLOYMENT_CONFIG` environment variable, or the XDG default
//! - Timestamped owner-only backup of a replaced deployment config
//! - The owner-only (0700) permission rule itself, as `restrict_dir`, which
//!   this module owns and other crates call
//!
//! ## Does not own
//! - Session or profile overlays
//! - Config path allowlisting
//! - The envelope format and the sealed-path list (see `secret_seal`)
//! - Which paths count as secret for display (see `secret`)
//! - The call sites that apply the owner-only rule (the CLI creates the backup
//!   directory; the caller decides when to restrict a path)
//!
//! ## Boundary
//!
//! Changes to deployment durability on disk should normally require changes here.
//! Changes to the owner-only permission rule should also require changes here.
//! Changes to the envelope format should not require changes here.
//! Changes to in-memory merge order should not require changes here.
//!
//! Keep this module focused on durable deployment overlay files and the
//! owner-only permission rule.
//!
//! # Sealing
//!
//! `save_overlay_file` is the one place the file is serialized, so it is the one
//! place a secret becomes unreadable on the way out, and `load_overlay_file` is
//! the one place it becomes readable again. Every write path goes through here —
//! `config_set` at `scope=deployment`, `flush_deployment_overlay`, and
//! `clio setup` — so none of them can accidentally write a secret in the clear.
//!
//! Sealing is a two-sided rule. On save, every value at a sealed path is
//! sealed, so the bytes on disk carry no readable secret. On load, a value at
//! a sealed path must be a sealed envelope: an unsealed string there is
//! refused, because a readable copy of that file would be a disclosure.
//! Ordinary paths stay plaintext in the file, and a read-only command never
//! rewrites the file, never takes a backup, and never touches its modification
//! time.
//!
//! The overlay in memory always holds plaintext, so masking, validation, and
//! every existing consumer behave exactly as they did before sealing existed.

use super::merge::{Overlay, flatten_object};
use crate::error::{AmError, ErrorCode};
use crate::secret_seal;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Env var naming the durable deployment overlay file.
pub const DEPLOYMENT_CONFIG_ENV: &str = "CLIO_DEPLOYMENT_CONFIG";

/// Default deployment overlay file name under the XDG config app directory.
pub const DEPLOYMENT_CONFIG_FILE_NAME: &str = "deployment.json";

/// Directory holding timestamped backups of replaced deployment configs.
const BACKUP_DIR_NAME: &str = "backups";

/// Monotonic per-process counter for temp-file names. PID plus nanos is not
/// enough on its own: two writers can read the same clock value, so the
/// counter guarantees distinct temp siblings even in the same nanosecond.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Resolve the deployment config path: explicit override, else
/// `CLIO_DEPLOYMENT_CONFIG` (an empty value disables loading), else the XDG
/// default under `$XDG_CONFIG_HOME/clio/` (fallback `$HOME/.config/clio/`), else
/// `None` when no home directory is known.
#[must_use]
pub fn resolve_deployment_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    resolve_deployment_path_with(explicit, |key| std::env::var(key).ok())
}

/// Injectable variant of [`resolve_deployment_path`]: the lookup drives both the
/// `CLIO_DEPLOYMENT_CONFIG` override and the XDG default, so tests never depend on
/// the host environment.
#[must_use]
pub(crate) fn resolve_deployment_path_with(
    explicit: Option<PathBuf>,
    get: impl Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    match get(DEPLOYMENT_CONFIG_ENV) {
        // An explicitly set-but-empty override disables deployment loading.
        Some(raw) if raw.trim().is_empty() => None,
        Some(raw) => Some(PathBuf::from(raw.trim())),
        None => default_deployment_path(&get),
    }
}

/// The XDG default deployment path, or `None` when neither `XDG_CONFIG_HOME`
/// nor `HOME` names a usable directory. Relative base directories are ignored
/// (XDG requires absolute paths).
#[must_use]
fn default_deployment_path(get: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    Some(default_config_dir(get)?.join(DEPLOYMENT_CONFIG_FILE_NAME))
}

/// The XDG config base directory (`$XDG_CONFIG_HOME/clio/`, fallback
/// `$HOME/.config/clio/`), or `None` when neither names an absolute path.
/// Shared by every store file that lives under the config directory.
#[must_use]
pub fn default_config_dir(get: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = nonempty(get("XDG_CONFIG_HOME"))
        .filter(|dir| Path::new(dir).is_absolute())
        .map(PathBuf::from)
        .or_else(|| {
            nonempty(get("HOME"))
                .filter(|home| Path::new(home).is_absolute())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join(crate::db_path::APP_DIR_NAME))
}

/// Trimmed, non-empty lookup value.
fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_owned())
        .filter(|v| !v.is_empty())
}

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

/// Persist overlay as a flat JSON object for stable round-trips.
///
/// Every value at a sealed path is sealed first, so the bytes on disk carry no
/// readable secret. A non-string leaf at a sealed path is left alone rather than
/// turned into a string: nothing writes a number or an object under a credential
/// path, and silently changing a value's type would be worse than leaving it.
///
/// Writes owner-only and atomically (temp file + rename), so a crash never
/// leaves a half-written config.
///
/// # Errors
/// A sealed value that cannot be sealed — which includes a Clio encryption key
/// that cannot be loaded, never a fallback to plaintext — or I/O failures
/// creating parent dirs or writing the file.
pub fn save_overlay_file(path: &Path, overlay: &Overlay) -> Result<(), AmError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| {
            AmError::new(
                ErrorCode::Internal,
                format!("failed to create {}: {e}", parent.display()),
            )
        })?;
    }
    let mut map = Map::new();
    for (k, v) in overlay {
        map.insert(k.clone(), seal_value(k, v, path)?);
    }
    // `Value` always serializes; treat failure as a bug rather than a soft error.
    let text = serde_json::to_string_pretty(&Value::Object(map)).expect("serialize overlay");
    write_private_atomic(path, &text).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to write deployment config {}: {e}", path.display()),
        )
    })
}

/// The stored form of one overlay value: sealed when the path calls for it.
///
/// # Errors
/// The value cannot be sealed, which includes the Clio encryption key not being
/// loadable. The message names the path and never the value.
fn seal_value(path: &str, value: &Value, file: &Path) -> Result<Value, AmError> {
    if !secret_seal::is_sealed_path(path) {
        return Ok(value.clone());
    }
    let Some(plain) = value.as_str() else {
        return Ok(value.clone());
    };
    secret_seal::seal(plain).map(Value::String).map_err(|err| {
        AmError::new(
            err.code,
            format!(
                "deployment config {}: cannot seal the value at `{path}`: {}. Clio wrote \
                     nothing, rather than writing this value in plaintext",
                file.display(),
                err.message
            ),
        )
    })
}

/// Back up `path` into a timestamped `backups/` sibling with owner-only
/// permissions. Returns the backup path, or `None` when `path` does not exist
/// (nothing to protect).
///
/// # Errors
/// I/O failures reading the source or writing the backup.
pub fn backup_config_file(path: &Path) -> Result<Option<PathBuf>, AmError> {
    if !path.exists() {
        return Ok(None);
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(BACKUP_DIR_NAME);
    fs::create_dir_all(&dir).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to create backup dir {}: {e}", dir.display()),
        )
    })?;
    restrict_dir(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(DEPLOYMENT_CONFIG_FILE_NAME);
    let target = dir.join(format!("{name}.{stamp}"));
    let text = fs::read_to_string(path).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to read {} for backup: {e}", path.display()),
        )
    })?;
    write_private(&target, &text).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to write backup {}: {e}", target.display()),
        )
    })?;
    Ok(Some(target))
}

/// Write `contents` to `path` atomically, owner-only on Unix. The temp file
/// lives beside the target so the final rename stays on one filesystem.
/// Crate-shared: the token store saves through the same writer.
///
/// # Errors
/// The temp file cannot be written, or the final rename fails (the temp file
/// is removed before returning).
pub fn write_private_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = temp_sibling(path);
    write_private(&tmp, contents)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn temp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(DEPLOYMENT_CONFIG_FILE_NAME);
    // PID + nanos + a monotonic counter keeps concurrent writers (threads or
    // processes) from clobbering each other's temp file before the rename.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{name}.{}.{}.{}.tmp",
        std::process::id(),
        nanos,
        seq
    ))
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // A pre-existing temp file keeps its old mode through `open`; force 0600.
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    fs::write(path, contents)
}

#[cfg(unix)]
pub fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub fn restrict_dir(_path: &Path) {}
#[cfg(test)]
#[path = "persist_seal_gate_tests.rs"]
mod seal_gate_tests;
#[cfg(test)]
#[path = "persist_seal_tests.rs"]
mod seal_tests;
#[cfg(test)]
#[path = "persist_seal_write_tests.rs"]
mod seal_write_tests;
#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "persist_db_agreement_tests.rs"]
mod persist_db_agreement_tests;
