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
//! - Refusing to write a sealed value in plaintext
//! - The stable flat-file shape a reload expects
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

use super::write::write_private_atomic;
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
