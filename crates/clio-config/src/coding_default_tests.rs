// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the builtin coding-agent
//! default profile: its validation, its strictness shape, and the defaults it
//! carries for the version-3 fields.
//!
//! ## Owns
//! - Validation of the coding default and its new-field values
//! - The selective/strict shape and the builtin mission staying in place
//!
//! ## Does not own
//! - The chatter fixture calibration (see `clio-admission`)
//! - Profile storage and resolution order (see `retention_store`)
//!
//! ## Boundary
//!
//! Changes to the coding default's field values should normally require changes
//! here. Changes to mission matching should not.
//!
//! Keep this module focused on the coding default profile.

use super::*;

#[test]
fn coding_default_validates_and_carries_the_new_field_defaults() {
    let profile = coding_default_profile();
    profile.validate().expect("coding default is valid");
    assert!(profile.retention_curve);
    assert!((profile.retention_floor - DEFAULT_RETENTION_FLOOR).abs() < 1e-12);
    assert_eq!(profile.pack_fidelity_default, PackFidelityDefault::Flow);
    assert_eq!(profile.provenance_floor_default, None);
    assert!(profile.maintenance_trigger);
    assert_eq!(profile.version, RETENTION_PROFILE_VERSION);
}

#[test]
fn coding_default_keeps_selective_strict_and_its_mission() {
    let profile = coding_default_profile();
    assert_eq!(profile.verbosity, Verbosity::Selective);
    assert_eq!(
        profile.duplicate_tolerance_write,
        DuplicateTolerance::Strict
    );
    assert_eq!(profile.recall_scope_default, RecallScope::Full);
    assert!(
        profile.mission.is_some(),
        "coding default carries a mission"
    );
    assert_eq!(
        profile.resolve().profile_version(),
        RETENTION_PROFILE_VERSION
    );
}
