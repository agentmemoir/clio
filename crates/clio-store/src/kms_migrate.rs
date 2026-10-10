// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the key store's v2 value shape: the sealed
//! form of a per-subject key.
//!
//! ## Owns
//! - `wrap_key_b64` / `unwrap_key_b64`, the boundary between a plaintext DEK
//!   and the bytes on disk
//!
//! ## Does not own
//! - The Clio encryption key's own lifecycle and the envelope itself (see
//!   `clio-config`'s `secret_key` and `secret_seal`)
//! - The SQLite schema, the format-version gate, and row operations (see
//!   `local_kms_db`)
//! - Store creation and installation (see `local_kms_migrate`)
//!
//! ## Boundary
//!
//! Changes to what a stored slot value looks like should normally require
//! changes here. Changes to the schema or the row operations should not require
//! changes here.
//!
//! Keep this module focused on the stored value shape.
//!
//! # What v2 is
//!
//! One envelope, the same one `clio-config` uses for a config secret: a
//! `sealed:v1:` string. A stored slot holds base64 sealed under the Clio
//! encryption key. The row shape does not change, so a half-written file is
//! caught by the format gate, and a shredded slot is still `NULL` and is never
//! wrapped or touched.
//!
//! The Clio key is not stored in the key-store file. Only the per-subject keys
//! are wrapped, which is what stops the file from being a place the key that
//! unwraps it is sitting in.

use clio_types::AmError;

/// The stored form of one plaintext slot value.
///
/// # Errors
/// The Clio encryption key cannot be loaded, or the cipher refuses the input.
pub(crate) fn wrap_key_b64(plain: &str) -> Result<String, AmError> {
    clio_config::secret_seal::seal(plain)
}

/// The plaintext form of one stored slot value.
///
/// The stored value must be a sealed envelope; the opener refuses anything
/// else, so a plaintext row is never silently read as if it were sealed.
///
/// # Errors
/// The value is not sealed at all, the envelope is malformed, or was sealed
/// under a different Clio key.
pub(crate) fn unwrap_key_b64(stored: &str) -> Result<String, AmError> {
    clio_config::secret_seal::open(stored)
}

/// The key every key-store test seals under.
///
/// Shared by the whole test binary on purpose: the concurrency tests seal on
/// threads they spawned, and a key installed for one thread would not reach them.
#[cfg(test)]
pub(crate) const TEST_KEY: [u8; clio_config::secret_key::KEY_LEN] =
    [0x5a; clio_config::secret_key::KEY_LEN];

/// Install [`TEST_KEY`] as the process key, so no key-store test reaches the
/// machine's real key store.
#[cfg(test)]
pub(crate) fn install_test_key() {
    clio_config::secret_seal::test_key::install_process_key(TEST_KEY);
}
