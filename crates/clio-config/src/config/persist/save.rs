// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for sealing and writing a deployment overlay to
//! disk as a flat JSON object.
//!
//! ## Owns
//! - The one serialization point, where a secret becomes unreadable on the way out
//! - Asking the sealing rule about each value, path and value together
//! - Refusing to write a sealed value in plaintext
//! - The stable flat-file shape a reload expects
//! - The checked save, which asks the guard to confirm the judged file just
//!   before the rename replaces the path
//!
//! ## Does not own
//! - Opening sealed values on the way in (see `load`)
//! - Atomic owner-only writes (see `write`)
//! - The pre-replace backup (see `backup`)
//!
//! ## Boundary
//!
//! Changes to what the on-disk file holds or how a value is sealed belong here.
//! Changes to how the file is loaded should not require changes here.
//!
//! Keep this module focused on sealing and writing the deployment overlay.

use super::load::CheckedOverwrite;
use super::write::{write_private_atomic, write_private_atomic_checked};
use crate::config::merge::Overlay;
use crate::error::{AmError, ErrorCode};
use crate::secret_seal;
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;

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
    let text = sealed_text(path, overlay)?;
    write_private_atomic(path, &text).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("failed to write deployment config {}: {e}", path.display()),
        )
    })
}

/// Persist overlay like [`save_overlay_file`], but run `check` just before the
/// rename that replaces the deployment file.
///
/// The check refuses the replacement when the path no longer holds the file it
/// judged. Nothing at the path changes on a refusal, and the temp file is
/// removed.
///
/// # Errors
/// A sealed value that cannot be sealed, I/O failures creating parent dirs or
/// writing the file, or the refusal the check returns.
pub(crate) fn save_overlay_file_checked(
    path: &Path,
    overlay: &Overlay,
    check: &CheckedOverwrite,
) -> Result<(), AmError> {
    let text = sealed_text(path, overlay)?;
    write_private_atomic_checked(path, &text, || check.ensure_unchanged(path))
}

/// Seal every value and return the bytes to write.
///
/// # Errors
/// A sealed value that cannot be sealed, or a parent directory that cannot be
/// created.
fn sealed_text(path: &Path, overlay: &Overlay) -> Result<String, AmError> {
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
    Ok(serde_json::to_string_pretty(&Value::Object(map)).expect("serialize overlay"))
}

/// The stored form of one overlay value: sealed when the rule calls for it.
///
/// The rule reads the value as well as the path, so a URL whose userinfo carries
/// a password is sealed wherever a user sets it, and not only under a path on the
/// sealed list. A value that is already an envelope passes through, which is what
/// keeps a second save of one overlay from sealing the same text twice.
///
/// # Errors
/// The value cannot be sealed, which includes the Clio encryption key not being
/// loadable. The message names the path and never the value.
fn seal_value(path: &str, value: &Value, file: &Path) -> Result<Value, AmError> {
    let Some(plain) = value.as_str() else {
        return Ok(value.clone());
    };
    if !secret_seal::must_seal(path, plain) {
        return Ok(value.clone());
    }
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

#[cfg(test)]
#[path = "save_tests.rs"]
mod save_tests;
