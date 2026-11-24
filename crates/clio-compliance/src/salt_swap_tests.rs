// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the deployment salt tests that pass a stale
//! path observation: a sidecar changed after it was checked, before it was read.
//!
//! ## Owns
//! - The refusals for a sidecar replaced or widened after the observation
//!
//! ## Does not own
//! - The salt resolution paths (`salt_tests.rs` owns those)
//! - The seeded state those tests start from (`salt_test_support.rs` owns that)
//!
//! ## Boundary
//!
//! Tests here break when the check and the read stop naming one file.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the swapped-sidecar refusal.

#![cfg(unix)]

use super::read_observed;
use crate::salt_test_support::{Scratch, salt_of, seed_owner_only, set_mode, sidecar};
use clio_types::ErrorCode;

/// A link planted after the observation is refused, even when it points at a
/// file this build would otherwise read: the descriptor the open returns names a
/// different file than the one that was checked, so the bytes read would not be
/// the bytes the owner-only rule cleared. Both the link and the file it names
/// are left exactly as they were found.
#[test]
fn a_sidecar_replaced_after_the_observation_is_refused() {
    let scratch = Scratch::new();
    let path = sidecar(&scratch.journal());
    seed_owner_only(&path, &salt_of(b'a', 32));
    let observed = std::fs::symlink_metadata(&path).expect("observe the seeded sidecar");
    let target = scratch.root.join("elsewhere.salt");
    let seeded_target = salt_of(b'b', 32);
    seed_owner_only(&target, &seeded_target);
    std::fs::remove_file(&path).expect("clear the observed sidecar");
    std::os::unix::fs::symlink(&target, &path).expect("plant a link at the salt path");
    let err = read_observed(&path, &observed).expect_err("a swapped sidecar is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("swapped after it was checked"),
        "the refusal names the swap: {}",
        err.message
    );
    assert!(
        !err.message.contains(&"a".repeat(32)) && !err.message.contains(&"b".repeat(32)),
        "the refusal carries no salt bytes: {}",
        err.message
    );
    assert!(
        std::fs::symlink_metadata(&path)
            .expect("inspect the planted link")
            .file_type()
            .is_symlink(),
        "the link is left exactly as it was found"
    );
    assert_eq!(
        std::fs::read(&target).expect("read the file the link named"),
        seeded_target,
        "the file the link named is never written through"
    );
}

/// A mode widened after the observation is refused from the opened
/// descriptor's own mode: the same file with a new mode is the file the check
/// no longer clears, and the refusal names the mode and never the salt.
#[test]
fn a_sidecar_widened_after_the_observation_is_refused() {
    let scratch = Scratch::new();
    let path = sidecar(&scratch.journal());
    let seeded = salt_of(b'a', 32);
    seed_owner_only(&path, &seeded);
    let observed = std::fs::symlink_metadata(&path).expect("observe the seeded sidecar");
    set_mode(&path, 0o644);
    let err = read_observed(&path, &observed).expect_err("a widened sidecar is refused");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{}", err.message);
    assert!(
        err.message.contains("0644"),
        "the refusal names the mode found: {}",
        err.message
    );
    assert!(
        !err.message.contains(&"a".repeat(32)),
        "the refusal carries no salt bytes: {}",
        err.message
    );
    assert_eq!(
        std::fs::read(&path).expect("read back"),
        seeded,
        "the refused sidecar is neither replaced nor regenerated"
    );
}
