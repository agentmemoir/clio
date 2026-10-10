// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for editing a harness's JSON config file that
//! Clio does not own: backing it up, refusing what it cannot parse, removing
//! Clio's MCP server entry, and replacing the file atomically without
//! disturbing any other entry.
//!
//! ## Owns
//! - Timestamped same-directory backup of a config before it is edited
//! - Parse-refusal: an unparseable file is left byte-identical, with a snippet
//! - Atomic replace through a same-directory temp sibling and rename
//! - Removing one named server entry under every caller-supplied key prefix
//!   that holds it
//! - Atomic replace through a same-directory temp sibling and rename: the
//!   temp file is created with the target mode, synced, then renamed
//!
//! ## Does not own
//! - Which harness uses which config file or key prefix; the caller decides
//! - Parsing or writing a non-JSON format; only JSON is handled
//! - Deciding when an edit runs, or reporting its result to the user
//!
//! ## Boundary
//!
//! Changes to how a foreign JSON config is backed up, refused, or rewritten
//! should normally require changes here. Changes to which harnesses exist or
//! where their config files live should not.
//!
//! Keep this module focused on the safe JSON edit of a file Clio does not own.

use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Why a config edit did not happen.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// The file is not valid JSON. It was not changed; `snippet` is what the
    /// user can act on by hand, plus the offending line's number. The line's
    /// bytes are never carried, so a refusal cannot copy the user's other
    /// entries out of the file.
    Unparseable { path: PathBuf, snippet: String },
    /// The file could not be read, backed up, or written. `detail` names the
    /// operation and path.
    Io { detail: String },
}

/// What happened to a requested removal.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RemoveOutcome {
    /// Clio's entry was present and has been removed. The backup holds the
    /// file's bytes before the edit.
    Removed { backup: PathBuf },
    /// No Clio entry was present, so nothing was written.
    Absent,
}

/// Back up `path` beside itself as `<name>.clio-backup-<unix_nanos>`.
///
/// Returns `Ok(None)` when there is no file to back up. The copy keeps the
/// source's bytes and permission bits, because `fs::copy` does.
///
/// # Errors
/// The copy fails.
pub(crate) fn backup_file(path: &Path) -> Result<Option<PathBuf>, WriteError> {
    if !path.exists() {
        return Ok(None);
    }
    let name = file_name(path);
    let nanos = unix_nanos();
    let target = path.with_file_name(format!("{name}.clio-backup-{nanos}"));
    fs::copy(path, &target).map_err(|e| io_error("back up", path, &e))?;
    Ok(Some(target))
}

/// Read and parse `path`, or `Ok(None)` when it does not exist.
///
/// # Errors
/// `Unparseable` when the bytes are not valid JSON, `Io` on a read failure.
pub(crate) fn read_json(path: &Path) -> Result<Option<Value>, WriteError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error("read", path, &e)),
    };
    match serde_json::from_str(&text) {
        Ok(value) => Ok(Some(value)),
        Err(e) => Err(WriteError::Unparseable {
            path: path.to_path_buf(),
            snippet: offending_line_note(e.line()),
        }),
    }
}

/// Remove `server_name` from every key prefix in `key_prefixes` that holds
/// it. Returns whether any entry was removed.
///
/// A prefix that does not resolve to a JSON object is skipped, so a caller can
/// pass a version-2 path and a version-1 fallback in one call. A config that
/// holds the server under two prefixes at once is fully cleared, so a later
/// read-back cannot find a leftover copy.
pub(crate) fn remove_server_entry(
    value: &mut Value,
    key_prefixes: &[&[&str]],
    server_name: &str,
) -> bool {
    let mut removed = false;
    for prefix in key_prefixes {
        let Some(map) = object_at_mut(value, prefix) else {
            continue;
        };
        removed |= map.remove(server_name).is_some();
    }
    removed
}

/// Write `value` to `path` atomically, keeping `path`'s permission bits.
///
/// The whole file is re-serialized with `serde_json`, so key order and
/// whitespace in the source may change: every value and entry is preserved by
/// parsed value, not byte-identical. The temp file is a sibling, so the rename
/// stays on one filesystem. It is created with `path`'s mode before any byte
/// is written, then synced to disk before the rename, so the file never exists
/// with wider permissions. A new file gets mode `0o644`. When `path` is a
/// symlink, its target is edited and the link itself is kept. On a rename
/// failure the temp file is removed.
///
/// # Errors
/// Serializing, creating the temp file, writing it, setting its mode,
/// syncing it, or the rename fails.
pub(crate) fn write_json_atomic(path: &Path, value: &Value) -> Result<(), WriteError> {
    let path = effective_path(path)?;
    let text = serde_json::to_string_pretty(value).map_err(|e| WriteError::Io {
        detail: format!("failed to serialize {}: {e}", path.display()),
    })?;
    let mode = existing_mode(&path).unwrap_or(0o644);
    let (tmp, file) = create_temp(&path, mode)?;
    if let Err(err) = write_and_sync(file, text.as_bytes(), mode, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(io_error("replace", &path, &e));
    }
    Ok(())
}

/// Remove Clio's entry from a JSON config file.
///
/// A missing file is `Ok(Absent)`. When no entry matches, nothing is written
/// and no backup is made. Otherwise the file is backed up first, the entry is
/// removed from every key prefix that holds it, and the result is written
/// atomically: a parseable file is re-serialized, so key order and whitespace
/// may change while every value and entry is preserved by parsed value, not
/// byte-identical. A symlinked config is followed to its target, so the link
/// itself survives. An unparseable file is refused byte-identical and its
/// refusal combines the caller's manual `snippet` with the offending line's
/// number, never the line's bytes.
///
/// # Errors
/// A read, backup, or write failure, or an unparseable file.
pub(crate) fn remove_server_from_file(
    path: &Path,
    key_prefixes: &[&[&str]],
    server_name: &str,
    snippet: &str,
) -> Result<RemoveOutcome, WriteError> {
    let path = effective_path(path)?;
    let mut value = match read_json(&path) {
        Ok(Some(value)) => value,
        Ok(None) => return Ok(RemoveOutcome::Absent),
        Err(WriteError::Unparseable {
            path: err_path,
            snippet: line_note,
        }) => {
            return Err(WriteError::Unparseable {
                path: err_path,
                snippet: format!("{snippet} {line_note}"),
            });
        }
        Err(err) => return Err(err),
    };
    if !remove_server_entry(&mut value, key_prefixes, server_name) {
        return Ok(RemoveOutcome::Absent);
    }
    let backup = backup_file(&path)?.ok_or_else(|| WriteError::Io {
        detail: format!("{} vanished before it could be backed up", path.display()),
    })?;
    write_json_atomic(&path, &value)?;
    Ok(RemoveOutcome::Removed { backup })
}

/// The target object under `prefix`, or `None` when the path is not all
/// objects. An empty prefix points at the root value itself.
fn object_at_mut<'a>(value: &'a mut Value, prefix: &[&str]) -> Option<&'a mut Map<String, Value>> {
    let target = if prefix.is_empty() {
        value
    } else {
        value.pointer_mut(&pointer_for(prefix))?
    };
    match target {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// A JSON Pointer for an unescaped key path.
fn pointer_for(prefix: &[&str]) -> String {
    let mut pointer = String::new();
    for key in prefix {
        pointer.push('/');
        pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
    }
    pointer
}

/// The offending-line note for an unparseable file: the parser's line number
/// only. The line's bytes are never printed, so a refusal cannot copy other
/// entries out of the user's config.
fn offending_line_note(line: usize) -> String {
    format!("(offending line: {line})")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json")
        .to_owned()
}

fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos())
}

/// A hidden temp sibling beside `path`, unique across threads and processes.
fn temp_sibling(path: &Path) -> PathBuf {
    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let name = file_name(path);
    let pid = std::process::id();
    let nanos = unix_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{pid}.{nanos}.{seq}.tmp"))
}

/// The file `path` names with symlinks resolved: when `path` itself is a
/// symlink, the edit follows it to its target so the link keeps working. A
/// missing file has nothing to resolve and is returned as-is.
fn effective_path(path: &Path) -> Result<PathBuf, WriteError> {
    let mut current = path.to_path_buf();
    for _ in 0..8 {
        let is_link = match fs::symlink_metadata(&current) {
            Ok(meta) => meta.file_type().is_symlink(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(current),
            Err(err) => return Err(io_error("inspect", &current, &err)),
        };
        if !is_link {
            return Ok(current);
        }
        let link = fs::read_link(&current).map_err(|e| io_error("resolve", &current, &e))?;
        current = if link.is_absolute() {
            link
        } else if let Some(parent) = current.parent() {
            parent.join(link)
        } else {
            link
        };
    }
    Err(WriteError::Io {
        detail: format!(
            "failed to resolve {}: too many symlink levels",
            path.display()
        ),
    })
}

/// Create a new temp sibling of `path` with `mode` already applied, so the
/// file never exists with wider permissions. The name is unique across threads
/// and processes; a leftover name from a crashed run is skipped.
fn create_temp(path: &Path, mode: u32) -> Result<(PathBuf, std::fs::File), WriteError> {
    for _ in 0..10 {
        let tmp = temp_sibling(path);
        match open_new(&tmp, mode) {
            Ok(file) => return Ok((tmp, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(io_error("create", &tmp, &err)),
        }
    }
    Err(WriteError::Io {
        detail: format!("failed to create a temp file beside {}", path.display()),
    })
}

#[cfg(unix)]
fn open_new(tmp: &Path, mode: u32) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(tmp)
}

#[cfg(not(unix))]
fn open_new(tmp: &Path, _mode: u32) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
}

/// Write `bytes` to the temp `file`, restore the exact mode in case the
/// process mask narrowed it at creation, and sync to disk before the caller
/// renames the temp file over its target.
fn write_and_sync(
    file: std::fs::File,
    bytes: &[u8],
    mode: u32,
    tmp: &Path,
) -> Result<(), WriteError> {
    use std::io::Write;
    let mut file = file;
    file.write_all(bytes)
        .map_err(|e| io_error("write", tmp, &e))?;
    set_mode(tmp, mode).map_err(|e| io_error("set permissions on", tmp, &e))?;
    file.sync_all().map_err(|e| io_error("sync", tmp, &e))?;
    Ok(())
}

fn io_error(action: &str, path: &Path, err: &std::io::Error) -> WriteError {
    WriteError::Io {
        detail: format!("failed to {action} {}: {err}", path.display()),
    }
}

#[cfg(unix)]
fn existing_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|meta| meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn existing_mode(_path: &Path) -> Option<u32> {
    None
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "agent_harness_write_tests.rs"]
mod tests;
