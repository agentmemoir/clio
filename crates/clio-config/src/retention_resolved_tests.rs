// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the resolved retention
//! snapshot's curve, pack channel, provenance floor, and maintenance trigger
//! values and accessors.
//!
//! ## Owns
//! - Builtin default values on the resolved snapshot
//! - Accessor return values for every carried field
//!
//! ## Does not own
//! - Profile parsing, validation, and resolution (see `retention`)
//! - Admission scoring that consumes the snapshot (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to the resolved snapshot fields or accessors should normally require
//! changes here. Changes to profile defaults should not.
//!
//! Keep this module focused on the resolved snapshot values.

use super::*;

#[test]
fn builtin_default_carries_the_new_documented_defaults() {
    let resolved = ResolvedRetention::builtin_default();
    assert_eq!(resolved, ResolvedRetention::default());
    assert!(resolved.retention_curve());
    assert!((resolved.retention_floor() - 0.25).abs() < 1e-12);
    assert_eq!(resolved.pack_fidelity_default(), PackFidelityDefault::Flow);
    assert_eq!(resolved.provenance_floor_default(), None);
    assert!(resolved.maintenance_trigger());
    assert_eq!(resolved.profile_version(), 3);
}

#[test]
fn accessors_return_the_carried_values() {
    let resolved = ResolvedRetention {
        retention_curve: false,
        retention_floor: 0.5,
        pack_fidelity_default: PackFidelityDefault::Exact,
        provenance_floor_default: Some(ProvenanceFloorDefault::Inferred),
        maintenance_trigger: false,
        ..ResolvedRetention::default()
    };
    assert!(!resolved.retention_curve());
    assert!((resolved.retention_floor() - 0.5).abs() < 1e-12);
    assert_eq!(resolved.pack_fidelity_default(), PackFidelityDefault::Exact);
    assert_eq!(
        resolved.provenance_floor_default(),
        Some(ProvenanceFloorDefault::Inferred)
    );
    assert!(!resolved.maintenance_trigger());
}
