// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync device id a runtime state carries:
//! the value that keys this host's row in the durable per-device ordering mark,
//! so the mark survives a restart instead of starting empty under a new key.
//!
//! ## Owns
//! - The resolution order: explicit override, then the stored setting, then one
//!   freshly minted id that is written before it is returned
//! - The `sync_device_id` settings key and the minted value's shape
//! - The ephemeral-store answer, which stays the per-process form
//!
//! ## Does not own
//! - Any store behavior (see `clio-store`); this module only calls
//!   `setting_get` / `setting_set` on the handle the caller already holds
//! - The per-device mark itself, its monotone update, or the freshness guard
//!   that reads it (see `clio-sync`)
//! - Where an override comes from; the environment read lives in
//!   [`resolve_from_env`], and tests call [`resolve`] directly
//!
//! ## Boundary
//!
//! Changes to how a device id is chosen or minted should normally require
//! changes here. Changes to how the mark is stored or compared should not.
//!
//! Keep this module free of environment reads outside `resolve_from_env`.

use clio_store::OpsStore;
use clio_types::AmError;

/// Settings key holding this installation's sync device id.
pub(crate) const SETTING_KEY: &str = "sync_device_id";

/// Environment variable naming an explicit device id for this process.
pub(crate) const ENV_OVERRIDE: &str = "CLIO_SYNC_DEVICE_ID";

/// Prefix on every device id this module returns, minted or fallback.
const PREFIX: &str = "device-";

/// Resolve the device id for one open.
///
/// Order: a non-empty `override_id`, then a non-empty stored value, then one
/// newly minted value written to `SETTING_KEY` before it is returned. An
/// `ephemeral` store cannot persist a row, so it answers the per-process form
/// and writes nothing.
///
/// # Errors
/// Storage failures from the settings read or write.
pub(crate) fn resolve(
    ops: &dyn OpsStore,
    ephemeral: bool,
    override_id: Option<&str>,
) -> Result<String, AmError> {
    if let Some(id) = override_id.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(id.to_owned());
    }
    if ephemeral {
        return Ok(process_device_id());
    }
    if let Some(stored) = ops.setting_get(SETTING_KEY)? {
        let stored = stored.trim();
        if !stored.is_empty() {
            return Ok(stored.to_owned());
        }
    }
    let minted = mint();
    ops.setting_set(SETTING_KEY, &minted)?;
    Ok(minted)
}

/// Resolve the device id for a deployment process, honouring the environment
/// override. The only environment read in this module.
///
/// # Errors
/// As [`resolve`].
pub(crate) fn resolve_from_env(ops: &dyn OpsStore, ephemeral: bool) -> Result<String, AmError> {
    let named = std::env::var(ENV_OVERRIDE).ok();
    resolve(ops, ephemeral, named.as_deref())
}

/// One fresh id: the prefix plus a URL-safe random secret.
///
/// The secret is the same generator the token store mints credentials with, so
/// the alphabet is base64url and the id stays opaque everywhere else.
fn mint() -> String {
    format!("{PREFIX}{}", clio_config::auth_tokens::generate_secret())
}

/// The per-process id an ephemeral store keeps, unchanged from before the
/// durable setting existed.
fn process_device_id() -> String {
    format!("{PREFIX}{}", std::process::id())
}

#[cfg(test)]
#[path = "sync_device_identity_tests.rs"]
mod sync_device_identity_tests;
