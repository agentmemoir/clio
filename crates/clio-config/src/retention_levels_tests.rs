// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the retention level enums and
//! their wire names: verbosity, duplicate tolerance, recall scope, pack
//! fidelity, and the provenance floor ladder.
//!
//! ## Owns
//! - Defaults and wire round trips for the level enums
//! - The closed provenance ladder, its strength order, and unknown-name refusals
//!
//! ## Does not own
//! - The profile fields that carry these values (see `retention`)
//! - The mapping numbers in the resolved snapshot (see `retention_resolved`)
//! - Which rungs a retrieval admits (`clio-retrieve` owns that predicate)
//!
//! ## Boundary
//!
//! Changes to level names or accepted wire values should normally require
//! changes here. Changes to profile storage should not.
//!
//! Keep this module focused on level enum wire behavior.

use super::*;

/// The declaration ordinal the derived `Ord` compares, one arm per rung.
///
/// The match carries no wildcard on purpose. The enum is declared in this
/// crate, so the compiler knows every variant, and a fifth one — inserted
/// mid-list or appended after `inferred` — leaves this function non-exhaustive
/// and stops the build instead of quietly widening the ladder.
const fn declared_ordinal(rung: ProvenanceFloorDefault) -> u8 {
    match rung {
        ProvenanceFloorDefault::Verified => 0,
        ProvenanceFloorDefault::Stated => 1,
        ProvenanceFloorDefault::ThirdParty => 2,
        ProvenanceFloorDefault::Inferred => 3,
    }
}

#[test]
fn pack_fidelity_wire_values_and_default() {
    assert_eq!(PackFidelityDefault::default(), PackFidelityDefault::Flow);
    assert_eq!(PackFidelityDefault::Flow.as_str(), "flow");
    assert_eq!(PackFidelityDefault::Exact.as_str(), "exact");
    assert_eq!(
        PackFidelityDefault::parse("flow").expect("flow"),
        PackFidelityDefault::Flow
    );
    assert_eq!(
        PackFidelityDefault::parse("exact").expect("exact"),
        PackFidelityDefault::Exact
    );
    assert_eq!(
        PackFidelityDefault::parse("lossless")
            .expect_err("unknown channel")
            .code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn provenance_floor_is_a_closed_four_rung_ladder() {
    for (raw, rung) in [
        ("verified", ProvenanceFloorDefault::Verified),
        ("stated", ProvenanceFloorDefault::Stated),
        ("third_party", ProvenanceFloorDefault::ThirdParty),
        ("inferred", ProvenanceFloorDefault::Inferred),
    ] {
        assert_eq!(ProvenanceFloorDefault::parse(raw).expect(raw), rung);
        assert_eq!(rung.as_str(), raw);
    }
    assert_eq!(
        ProvenanceFloorDefault::parse("trusted")
            .expect_err("unknown rung")
            .code,
        ErrorCode::InvalidArgument
    );
}

/// The ladder is exactly these four rungs, and each keeps the declaration
/// ordinal `Ord` compares. A mid-list insertion shifts two ordinals and an
/// appended one shifts none, so the exhaustive `declared_ordinal` match and
/// the count below both have to be broken before a fifth rung reaches the wire.
#[test]
fn provenance_floor_has_exactly_four_rungs_with_pinned_ordinals() {
    let ladder = [
        (ProvenanceFloorDefault::Verified, "verified", 0_u8),
        (ProvenanceFloorDefault::Stated, "stated", 1),
        (ProvenanceFloorDefault::ThirdParty, "third_party", 2),
        (ProvenanceFloorDefault::Inferred, "inferred", 3),
    ];
    for (rung, wire, ordinal) in ladder {
        assert_eq!(
            rung as u8, ordinal,
            "`{wire}` keeps its declaration ordinal"
        );
        assert_eq!(declared_ordinal(rung), ordinal, "`{wire}` table ordinal");
        assert_eq!(rung.as_str(), wire, "`{wire}` wire name");
    }
    assert_eq!(
        declared_ordinal(ProvenanceFloorDefault::Inferred) as usize + 1,
        ladder.len(),
        "the ladder ends at `inferred`, so four rungs cover every ordinal"
    );
}

/// The refusal side of a closed ladder: a name outside the four rungs is an
/// `invalid_argument` rather than an alias, and the message names exactly the
/// four accepted wire names, so a new parseable rung has to move that list.
#[test]
fn provenance_floor_parse_refuses_a_name_outside_the_ladder() {
    for raw in ["trusted", "thirdparty", "Verified", "assumed"] {
        let error = ProvenanceFloorDefault::parse(raw).expect_err(raw);
        assert_eq!(error.code, ErrorCode::InvalidArgument, "for `{raw}`");
        assert!(
            error.message.contains(raw),
            "the refusal repeats what it read: {}",
            error.message
        );
    }
    assert_eq!(
        ProvenanceFloorDefault::parse("")
            .expect_err("an empty rung")
            .code,
        ErrorCode::InvalidArgument,
        "an empty name is refused rather than read as the loosest rung"
    );
    assert_eq!(
        ProvenanceFloorDefault::parse("assumed")
            .expect_err("a fifth rung")
            .message,
        "provenance_floor_default must be verified|stated|third_party|inferred, got `assumed`",
        "the refusal lists exactly the four accepted wire names"
    );
}

/// The declared order IS the strength order: `verified` is the strictest floor
/// and `inferred` the loosest, so each rung's admitted set sits inside the next
/// one's. A reader compares two rungs with `>=`, so a reordering would silently
/// invert every floor. This test compares only the four rungs it lists, so
/// `provenance_floor_has_exactly_four_rungs_with_pinned_ordinals` is what owns
/// the claim that the ladder has no fifth rung.
#[test]
fn provenance_floor_rungs_are_ordered_from_strictest_to_loosest() {
    let rungs = [
        ProvenanceFloorDefault::Verified,
        ProvenanceFloorDefault::Stated,
        ProvenanceFloorDefault::ThirdParty,
        ProvenanceFloorDefault::Inferred,
    ];
    for (index, rung) in rungs.iter().enumerate() {
        assert!(
            *rung >= ProvenanceFloorDefault::Verified,
            "{rung:?} is not at least as strict as `verified`"
        );
        assert!(
            *rung <= ProvenanceFloorDefault::Inferred,
            "{rung:?} is not at most as loose as `inferred`"
        );
        if let Some(next) = rungs.get(index + 1) {
            assert!(
                *rung < *next,
                "{rung:?} must sit strictly below {next:?} in strength order"
            );
        }
    }
}

#[test]
fn level_enums_serialize_their_snake_case_wire_names() {
    assert_eq!(
        serde_json::to_value(Verbosity::Selective).expect("json"),
        serde_json::json!("selective")
    );
    assert_eq!(
        serde_json::to_value(DuplicateTolerance::Strict).expect("json"),
        serde_json::json!("strict")
    );
    assert_eq!(
        serde_json::to_value(RecallScope::ConsolidatedOnly).expect("json"),
        serde_json::json!("consolidated_only")
    );
    assert_eq!(
        serde_json::to_value(PackFidelityDefault::Exact).expect("json"),
        serde_json::json!("exact")
    );
    assert_eq!(
        serde_json::to_value(ProvenanceFloorDefault::ThirdParty).expect("json"),
        serde_json::json!("third_party")
    );
}
