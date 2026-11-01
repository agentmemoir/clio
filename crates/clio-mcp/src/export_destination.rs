// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for confining the parent directory of a
//! caller-supplied export destination inside the deployment-owned export root.
//!
//! ## Owns
//! - Creating the export root owner-only (mode 0700 at creation) when it is
//!   absent, and refusing a root that resolves to a non-directory
//! - The filesystem-free lexical refusal of a destination whose parent is
//!   outside the root, and canonical containment of a parent that exists
//! - The refusals an outside destination, an unconfigured root, and a missing
//!   parent directory receive
//!
//! ## Does not own
//! - Resolving which directory is the export root (`clio_config::export_path`)
//! - Protecting the final path component: a symlink or non-regular file at the
//!   destination is refused by the compliance delivery layer, downstream of
//!   this gate
//! - Bundle contents, custody rows, or the file write (`clio-compliance`)
//! - MCP argument decoding or result shaping (`portability_tools`)
//!
//! ## Boundary
//!
//! Changes to how a destination's parent is checked against the export root
//! should normally require changes here. Changes to what an export contains, or
//! to where the root lives, should not require changes here.
//!
//! Only the destination's parent directory is confined here. The final path
//! component's symlink and non-regular-file protection lives downstream in the
//! compliance delivery layer.
//!
//! Keep this module focused on destination confinement.

use clio_types::{AmError, ErrorCode};
use std::path::{Component, Path, PathBuf};

/// The single refusal every lexically outside destination receives. It names
/// neither path, so it cannot be used to probe the deployment layout.
const OUTSIDE: &str = "export destination is outside the export root";

/// The single refusal every destination with a missing parent receives.
const NO_PARENT: &str = "export destination parent directory does not exist";

/// Resolve `destination` to an absolute path whose parent is inside `root`.
///
/// A relative destination is joined to `root`; an absolute one is taken as
/// given. The candidate's parent is normalized lexically and compared with a
/// lexically normalized root as whole path components, so a `..`-traversing
/// parent is refused before any filesystem call: an arbitrary outside path
/// cannot then be used to learn whether a directory exists, and the refusal
/// names neither the root nor the destination. Only after that shape gate does
/// the root itself get inspected.
///
/// A missing root is created owner-only (0700 at creation, never chmod-ed
/// afterwards) and accepts only a bare relative file name, because no other
/// shape can be proven to land in a directory that does not exist yet. An
/// existing root may be a plain directory or a symlink that resolves to one; a
/// root that resolves to anything else is a deployment fault, not a caller
/// error, so it reports [`ErrorCode::Internal`]. `root_trusted` is false for a
/// root the deployment did not choose (the relative fallback tier, which a
/// stdio server inherits from a client project's working directory); such a
/// root is refused as unconfigured.
///
/// Confinement is a validation gate, not a rewrite: the path returned is the
/// candidate the caller supplied, not its canonical form. The root and the
/// parent are canonicalized only to prove containment, then discarded, because
/// the destination identity is what the caller asked for and what the custody
/// row records, and the write must land on the path the caller chose. Returning
/// the canonical form would change that identity wherever the parent is reached
/// through a symlink — the system temp directory on macOS is reached through
/// `/var` -> `/private/var` — so the recorded destination would no longer match
/// the destination the caller holds.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] when the destination is empty, names no file,
/// names a parent directory that does not exist, or lies outside `root`;
/// [`ErrorCode::Internal`] when the root is unconfigured or is not a directory,
/// or when the root or the parent cannot be inspected, created, or
/// canonicalized.
pub(crate) fn confine_export_destination(
    root: &Path,
    root_trusted: bool,
    destination: &str,
) -> Result<PathBuf, AmError> {
    if destination.trim().is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "export destination must not be empty",
        ));
    }
    let requested = Path::new(destination);
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    if candidate.file_name().is_none() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "export destination names no file",
        ));
    }
    let root_lexical = normalize_lexically(root);
    let parent = candidate.parent().unwrap_or(root);
    let parent_lexical = normalize_lexically(parent);
    if !parent_lexical.starts_with(&root_lexical) {
        return Err(AmError::new(ErrorCode::InvalidArgument, OUTSIDE));
    }
    if !root_trusted {
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "the export root is not configured; set {} to a deployment-owned directory",
                clio_config::EXPORT_ROOT_ENV
            ),
        ));
    }
    let root_existed = match std::fs::symlink_metadata(root) {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(internal("inspect the export root", root, &err)),
    };
    if root_existed {
        require_directory(root)?;
    } else {
        if parent_lexical != root_lexical {
            return Err(AmError::new(ErrorCode::InvalidArgument, NO_PARENT));
        }
        create_root(root)?;
        require_directory(root)?;
    }
    let root_canonical = root
        .canonicalize()
        .map_err(|err| internal("canonicalize the export root", root, &err))?;
    // The parent did not exist at the shape gate only when the root itself was
    // absent, and the root has just been created: it is the canonical parent.
    let parent_canonical = if root_existed {
        parent.canonicalize().map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                AmError::new(ErrorCode::InvalidArgument, NO_PARENT)
            } else {
                internal("canonicalize the export destination parent", parent, &err)
            }
        })?
    } else {
        root_canonical.clone()
    };
    if !parent_canonical.starts_with(&root_canonical) {
        return Err(AmError::new(ErrorCode::InvalidArgument, OUTSIDE));
    }
    // Containment is proven; the caller's own candidate is what is returned, so
    // the destination identity the caller holds and the custody row records is
    // the one they supplied.
    Ok(candidate)
}

/// Drop `.` and resolve `..` by popping components, touching no filesystem.
///
/// A path that climbs above its root component stops at that component, which
/// only ever widens nothing: the caller's parent still has to contain the
/// root's own components to be accepted.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Create the export root owner-only, at creation time.
///
/// The mode is set as the directory is created rather than narrowed afterwards:
/// a `chmod` after `create_dir_all` would apply to whatever the path resolves to
/// by then, including a link a concurrent process swapped in.
#[cfg(unix)]
fn create_root(root: &Path) -> Result<(), AmError> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(|err| internal("create the export root", root, &err))
}

/// Create the export root where the process umask decides the mode.
#[cfg(not(unix))]
fn create_root(root: &Path) -> Result<(), AmError> {
    std::fs::create_dir_all(root).map_err(|err| internal("create the export root", root, &err))
}

/// Refuse a root that does not resolve to a directory.
///
/// The root is a deployment setting, so a wrong shape is an internal fault in
/// the configuration rather than an argument error the caller can correct. A
/// symlink to a directory is accepted: where the deployment points the root is
/// the deployment's business. A pre-existing directory is never re-chmodded, so
/// a root the deployment already set keeps the mode it was opened with.
fn require_directory(root: &Path) -> Result<(), AmError> {
    std::fs::symlink_metadata(root)
        .map_err(|err| internal("inspect the export root", root, &err))?;
    let is_directory = std::fs::metadata(root).is_ok_and(|metadata| metadata.is_dir());
    if !is_directory {
        return Err(AmError::new(
            ErrorCode::Internal,
            format!(
                "the configured export root `{}` is not a directory; refusing to deliver outside it",
                root.display()
            ),
        ));
    }
    Ok(())
}

/// One filesystem failure, reported as internal with the path it applies to.
fn internal(action: &str, path: &Path, err: &std::io::Error) -> AmError {
    AmError::new(
        ErrorCode::Internal,
        format!("{action} `{}`: {err}", path.display()),
    )
}

#[cfg(test)]
#[path = "export_destination_tests.rs"]
mod export_destination_tests;
