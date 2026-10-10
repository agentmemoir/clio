// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the archive member-name validation gate.
//!
//! ## Owns
//! - The single gate every member name passes before a header is built or read
//! - The rejection rules: empty, leading `/`, backslash, empty/`.`/`..`
//!   segment, non-printable-ASCII byte, and the 100-byte name field limit
//!
//! ## Does not own
//! - Header construction or encoding (see `container`)
//! - Manifest field validation (see `manifest`)
//!
//! ## Boundary
//!
//! Changes to which names are legal should normally require changes here.
//! Changes to the container layout should not require changes here.
//!
//! Keep this module focused on member-name validation.

use clio_types::{AmError, ErrorCode};

/// The ustar `name` field is 100 bytes; a legal member name fits it.
pub const MAX_NAME_BYTES: usize = 100;

/// Reject any member name this build refuses to store in an archive.
///
/// The gate never sanitizes: a name either passes unchanged or the caller
/// gets a specific error. The writer calls this before any header is built
/// and the reader calls it before any header is used.
///
/// Rejections: empty name; a leading `/`; any backslash byte; any empty,
/// `.`, or `..` path segment; any byte outside printable ASCII
/// (`0x20..=0x7E`); any name over [`MAX_NAME_BYTES`] bytes.
pub fn validate_member_name(name: &str) -> Result<(), AmError> {
    let reject = |reason: String| {
        Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("invalid archive member name {name:?}: {reason}"),
        ))
    };

    if name.is_empty() {
        return reject("the name is empty".into());
    }
    if name.starts_with('/') {
        return reject("the name starts with '/'".into());
    }
    if name.as_bytes().contains(&b'\\') {
        return reject("the name contains a backslash".into());
    }
    if let Some(&byte) = name
        .as_bytes()
        .iter()
        .find(|&&b| !(0x20..=0x7E).contains(&b))
    {
        return reject(format!(
            "byte 0x{byte:02x} is outside printable ASCII (0x20..=0x7E)"
        ));
    }
    for segment in name.split('/') {
        match segment {
            "" => return reject("the name has an empty path segment".into()),
            "." => return reject("the name has a '.' path segment".into()),
            ".." => return reject("the name has a '..' path segment".into()),
            _ => {}
        }
    }
    if name.len() > MAX_NAME_BYTES {
        return reject(format!(
            "the name is {} bytes, over the {}-byte limit",
            name.len(),
            MAX_NAME_BYTES
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "name_tests.rs"]
mod tests;
