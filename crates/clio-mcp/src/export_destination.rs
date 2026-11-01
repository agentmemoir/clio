// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for confining a caller-supplied export
//! destination inside the deployment-owned export root.
//!
//! ## Owns
//! - Creating the export root owner-only when it is absent, and refusing a root
//!   that is not a plain directory
//! - Canonical containment of a destination's parent directory under that root
//! - The refusal a destination outside the root receives
//!
//! ## Does not own
//! - Resolving which directory is the export root (`clio_config::export_path`)
//! - Bundle contents, custody rows, or the file write (`clio-compliance`)
//! - MCP argument decoding or result shaping (`portability_tools`)
//!
//! ## Boundary
//!
//! Changes to how a destination is checked against the export root should
//! normally require changes here. Changes to what an export contains, or to
//! where the root lives, should not require changes here.
//!
//! Keep this module focused on destination confinement.

use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};

/// Resolve `destination` to an absolute path inside `root`.
///
/// A relative destination is joined to `root`; an absolute one is taken as
/// given. The root is created owner-only when it is absent and its mode is
/// never changed when it already exists, so a deployment directory and a test
/// directory both keep the mode they were opened with. The destination's parent
/// directory is canonicalized and must be the root or below it, compared as
/// whole path components rather than as a string prefix, so a symlinked or
/// `..`-traversing parent that resolves elsewhere is refused. The final file
/// name is not canonicalized: the file need not exist yet.
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
/// names a parent directory that does not exist, or resolves outside `root`;
/// [`ErrorCode::Internal`] when the root or the parent directory cannot be
/// inspected, created, or canonicalized.
pub(crate) fn confine_export_destination(
    root: &Path,
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
    ensure_root(root)?;
    let root_canonical = root
        .canonicalize()
        .map_err(|err| internal("canonicalize the export root", root, &err))?;
    if candidate.file_name().is_none() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "export destination `{}` names no file inside the export root",
                requested.display()
            ),
        ));
    }
    let Some(parent) = candidate.parent() else {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "export destination `{}` names no parent directory",
                requested.display()
            ),
        ));
    };
    let parent_canonical = parent.canonicalize().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "export destination parent `{}` does not exist",
                    parent.display()
                ),
            )
        } else {
            internal("canonicalize the export destination parent", parent, &err)
        }
    })?;
    if !parent_canonical.starts_with(&root_canonical) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "export destination `{}` is outside the export root `{}`",
                requested.display(),
                root.display()
            ),
        ));
    }
    // Containment is proven; the caller's own candidate is what is returned, so
    // the destination identity the caller holds and the custody row records is
    // the one they supplied.
    Ok(candidate)
}

/// Ensure the export root exists as a plain directory, creating it owner-only.
///
/// A pre-existing directory is never re-chmodded: the root may be one the
/// deployment already set, and on a test machine it may be the system temp
/// directory, whose mode is not ours to narrow.
fn ensure_root(root: &Path) -> Result<(), AmError> {
    match std::fs::symlink_metadata(root) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(root)
                .map_err(|err| internal("create the export root", root, &err))?;
            clio_config::config::restrict_dir(root);
        }
        Err(err) => return Err(internal("inspect the export root", root, &err)),
    }
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|err| internal("inspect the export root", root, &err))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_dir() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "export root `{}` is not a plain directory; refusing to deliver outside it",
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
