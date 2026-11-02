// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the sync device id resolver: an
//! absent variable means no override, a usable value is returned trimmed, and an
//! unusable value is treated as absent.
//!
//! ## Owns
//! - Each accepted and rejected shape of `CLIO_SYNC_DEVICE_ID`
//!
//! ## Does not own
//! - The resolver implementation (see `sync_device_id`)
//! - Minting or storing the device id (`clio-store`)
//!
//! ## Boundary
//!
//! Tests here break when the override contract changes; they never change the
//! implementation.
//!
//! Keep this module focused on resolver unit behavior.

use super::*;

/// Build an injected environment lookup holding one value for the override key.
fn env(value: Option<&'static str>) -> impl Fn(&str) -> Option<String> + 'static {
    move |key: &str| {
        if key == SYNC_DEVICE_ID_ENV {
            value.map(str::to_owned)
        } else {
            None
        }
    }
}

#[test]
fn unset_variable_gives_no_override() {
    assert_eq!(resolve_sync_device_id(&env(None)), None);
}

#[test]
fn set_variable_gives_the_override() {
    assert_eq!(
        resolve_sync_device_id(&env(Some("device-abc"))),
        Some("device-abc".to_owned())
    );
}

#[test]
fn whitespace_only_value_is_absent() {
    assert_eq!(resolve_sync_device_id(&env(Some("   "))), None);
}

#[test]
fn interior_nul_is_absent() {
    assert_eq!(resolve_sync_device_id(&env(Some("device-\0abc"))), None);
}

#[test]
fn value_is_trimmed() {
    assert_eq!(
        resolve_sync_device_id(&env(Some("  device-abc\n"))),
        Some("device-abc".to_owned())
    );
}
