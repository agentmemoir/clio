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
//! - Timestamped same-directory backup of a config before it is edited, created
//!   exclusively so an existing entry or a planted symlink can never be
//!   followed or overwritten, and created with the source's mode already
//!   applied so the copy is never readable by anyone the source is not
//! - Parse-refusal: an unparseable file is left byte-identical, with a snippet
//! - Removing one named server entry under every caller-supplied key prefix
//!   that holds it
//! - Resolving a symlinked target once, so the link itself survives the edit
//!
//! ## Does not own
//! - Which harness uses which config file or key prefix; the caller decides
//! - Parsing or writing a non-JSON format; only JSON is handled
//! - Judging and replacing one entry through a directory descriptor; see
//!   `agent_harness_atomic_write`
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

use crate::agent_harness_atomic_write::AnchoredTarget;

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
/// Returns `Ok(None)` when there is no file to back up. The backup keeps the
/// source's bytes and permission bits, and is created with those bits already
/// applied, so it is never readable by anyone the source is not — not even
/// while it is being written. It is created exclusively: any entry already
/// holding the stamped name — a regular file, a directory, a symlink, or a
/// dangling symlink — makes the backup fail, so the bytes are never written
/// through a link and an existing entry is never overwritten, even by a second
/// backup stamped in the same nanosecond.
///
/// # Errors
/// The backup cannot be created exclusively, read from the source, written, or
/// given the source's permission bits; or an entry already exists at the
/// stamped name.
pub(crate) fn backup_file(path: &Path) -> Result<Option<PathBuf>, WriteError> {
    backup_file_at(path, unix_nanos())
}

/// Back up `path` under the stamp `nanos` rather than the clock's.
///
/// The stamp is `backup_file`'s only input that cannot be observed before the
/// call, so it is taken as a parameter and the name it builds can be known in
/// advance. Nothing else differs from `backup_file`: the same name is built
/// beside the source, the same exclusive create runs, and the same `Ok(None)`
/// applies when there is no file. This is the seam that lets a test plant an
/// entry at the exact name the shipped path will choose and watch the shipped
/// path refuse it; a test that had to guess the clock's stamp could only ever
/// exercise a fresh name.
///
/// Private to this module: a stamp is not part of the caller's contract, and
/// `backup_file` is the entry point they get.
fn backup_file_at(path: &Path, nanos: u128) -> Result<Option<PathBuf>, WriteError> {
    if !path.exists() {
        return Ok(None);
    }
    let name = file_name(path);
    let target = path.with_file_name(format!("{name}.clio-backup-{nanos}"));
    write_backup(path, &target)?;
    Ok(Some(target))
}

/// Copy `source` to `target`, which must not already exist, keeping the
/// source's permission bits.
///
/// The source is opened once and its permission bits are read from that
/// descriptor, so the bits written onto the backup are the bits of the file the
/// bytes came from, never those of a path resolved again later. The backup is
/// created with those bits already applied, so it never exists with a wider
/// mode than the source at any instant.
///
/// The create is exclusive, so an entry that is already there — whatever its
/// type, and whether or not a symlink resolves — fails the call instead of
/// being overwritten or followed. A later failure removes exactly the entry
/// this call created and nothing else.
fn write_backup(source: &Path, target: &Path) -> Result<(), WriteError> {
    let mut from = fs::File::open(source).map_err(|e| io_error("back up", source, &e))?;
    let permissions = from
        .metadata()
        .map_err(|e| io_error("back up", source, &e))?
        .permissions();
    let created = create_backup(target, &permissions)?;
    match fill_backup(&mut from, target, created, &permissions) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(target);
            Err(err)
        }
    }
}

/// Fill the already-created `target` with the bytes read from `from`, sync
/// them, and give it `permissions` exactly.
///
/// `created` is consumed, so the descriptor is closed before the caller can
/// remove the file on a failure. The mode is set on the open descriptor, not on
/// the path, so no second lookup can be pointed at another entry between the
/// write and the mode change. Applying it again is not redundant: the process
/// mask narrows the mode the create asked for, so only this restores the
/// source's exact bits.
fn fill_backup(
    from: &mut fs::File,
    target: &Path,
    mut created: fs::File,
    permissions: &fs::Permissions,
) -> Result<(), WriteError> {
    std::io::copy(from, &mut created).map_err(|e| io_error("back up", target, &e))?;
    created
        .sync_all()
        .map_err(|e| io_error("back up", target, &e))?;
    created
        .set_permissions(permissions.clone())
        .map_err(|e| io_error("back up", target, &e))
}

/// Create `target` exclusively with `permissions`' bits already applied, so the
/// entry never exists with a wider mode than the source it will hold.
///
/// `O_CREAT | O_EXCL` and the mode are one syscall, so there is no instant
/// between the create and the mode change in which another local user could
/// open the file. `O_EXCL` also refuses a symlink at the name rather than
/// following it, so a planted link reaches nothing.
#[cfg(unix)]
fn create_backup(target: &Path, permissions: &fs::Permissions) -> Result<fs::File, WriteError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(permissions.mode())
        .open(target)
        .map_err(|e| io_error("back up", target, &e))
}

/// Create `target` exclusively. This platform has no mode bits to apply at
/// creation, so exclusivity is the whole of the guarantee here.
#[cfg(not(unix))]
fn create_backup(target: &Path, _permissions: &fs::Permissions) -> Result<fs::File, WriteError> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| io_error("back up", target, &e))
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

/// Write `value` to `path` atomically, keeping the target's permission bits.
///
/// The file is re-serialized with `serde_json`, so key order and whitespace may
/// change while every value is preserved by parsed value. The temp sibling is
/// created with the target's mode before any byte is written and synced before
/// the rename, so the file never exists with wider permissions. A new file gets
/// mode `0o644`; a symlinked path edits its target and keeps the link; a final
/// entry that is not a regular file is refused; a failed write leaves no temp.
///
/// # Errors
/// Serializing, opening the parent directory, inspecting the final entry,
/// creating the temp file, writing it, setting its mode, syncing it, or the
/// rename fails.
pub(crate) fn write_json_atomic(path: &Path, value: &Value) -> Result<(), WriteError> {
    write_json_atomic_resolved(&effective_path(path)?, value)
}

/// Write `value` to `path`, which already has its symlinks resolved.
///
/// The resolved target is held once: on Unix the parent directory descriptor
/// plus the final entry name, elsewhere the path. On Unix every step after the
/// open uses the descriptor and the entry name, so the entry that is judged is
/// the entry that is replaced. A swap of a parent component before the open is
/// out of scope: it needs write access above the target directory, and it gains
/// no privilege the attacker does not already hold.
///
/// # Errors
/// Serializing the value fails, or the atomic write fails.
fn write_json_atomic_resolved(path: &Path, value: &Value) -> Result<(), WriteError> {
    let target = AnchoredTarget::open(path)?;
    write_value(&target, path, value)
}

/// Serialize `value` and write it through `target`, the entry already held
/// open.
fn write_value(target: &AnchoredTarget, path: &Path, value: &Value) -> Result<(), WriteError> {
    let text = serde_json::to_string_pretty(value).map_err(|e| WriteError::Io {
        detail: format!("failed to serialize {}: {e}", path.display()),
    })?;
    target.replace(text.as_bytes())
}

/// Remove Clio's entry from a JSON config file.
///
/// A missing file is `Ok(Absent)`. When no entry matches, nothing is written
/// and no backup is made. Otherwise the file is backed up first, the entry is
/// removed from every key prefix that holds it, and the result is written
/// atomically: a parseable file is re-serialized, so key order and whitespace
/// may change while every value and entry is preserved by parsed value, not
/// byte-identical. A symlinked config is followed to its target, so the link
/// itself survives; the final entry is resolved once and replaced through the
/// handle opened for it (a directory descriptor on Unix), so an entry swapped
/// in after the resolution cannot redirect the write. An unparseable file is
/// refused byte-identical and its refusal combines the caller's manual
/// `snippet` with the offending line's number, never the line's bytes.
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
    let target = AnchoredTarget::open(&path)?;
    let backup = backup_file(&path)?.ok_or_else(|| WriteError::Io {
        detail: format!("{} vanished before it could be backed up", path.display()),
    })?;
    write_value(&target, &path, &value)?;
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

/// The base name of `path`, for naming a backup or a temp sibling beside it.
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json")
        .to_owned()
}

/// The current time in nanoseconds since the Unix epoch, or zero before it.
pub(crate) fn unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos())
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

/// A filesystem error named with the path it concerns.
pub(crate) fn io_error(action: &str, path: &Path, err: &std::io::Error) -> WriteError {
    WriteError::Io {
        detail: format!("failed to {action} {}: {err}", path.display()),
    }
}

#[cfg(test)]
#[path = "agent_harness_write_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "agent_harness_write_toctou_tests.rs"]
mod toctou_tests;

#[cfg(all(test, unix))]
#[path = "agent_harness_write_backup_tests.rs"]
mod backup_tests;

#[cfg(all(test, unix))]
#[path = "agent_harness_write_backup_entry_tests.rs"]
mod backup_entry_tests;
