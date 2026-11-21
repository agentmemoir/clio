// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for sealing a readable export payload to one
//! recipient's age X25519 public key, so no readable byte of a DSAR bundle
//! reaches disk.
//!
//! ## Owns
//! - `seal_to_recipient`: recipient parsing and age v1 encryption
//!
//! ## Does not own
//! - When a seal is required and which mode is readable (`crate::export`)
//! - Bundle payload construction (`crate::export_subject`)
//!
//! ## Boundary
//!
//! Changes to how a payload is sealed should normally require changes here.
//! Changes to what a readable payload contains should not.
//!
//! Keep this module focused on the recipient seal.

use clio_types::{AmError, ErrorCode};

/// Seal `bytes` to an age X25519 `recipient` and return the age v1 file body.
///
/// The caller passes the exact recipient text the operator supplied. Malformed
/// recipient text is an argument error, so no destination is staged.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] for malformed recipient text, and
/// [`ErrorCode::Internal`] when the age encryptor fails.
pub(crate) fn seal_to_recipient(bytes: &[u8], recipient: &str) -> Result<Vec<u8>, AmError> {
    let recipient: age::x25519::Recipient = recipient.trim().parse().map_err(|_| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "recipient must be an age X25519 public key (age1...)",
        )
    })?;
    age::encrypt(&recipient, bytes).map_err(|error| {
        AmError::new(
            ErrorCode::Internal,
            format!("age recipient seal failed: {error}"),
        )
    })
}

#[cfg(test)]
#[path = "export_seal_tests.rs"]
mod export_seal_tests;
