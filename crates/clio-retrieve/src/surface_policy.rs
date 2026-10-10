// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving one bank retention profile's
//! read-side request fields from a call's wire arguments: the recall scope, the
//! read-time duplicate tolerance, the curve state, the provenance floor, and
//! the pack fidelity channel.
//!
//! ## Owns
//! - `resolve_policy`: an explicit argument wins, otherwise the profile default
//!   applies, and a value outside a closed set is refused rather than ignored
//! - `ResolvedReadPolicy`: the read-side fields one profile resolves to
//! - `parse_axis` and `invalid`: the two shared argument-parsing helpers the
//!   handlers call
//! - The wire-argument name each refusal is remapped to (`FLOOR_ARGUMENT` and
//!   `FIDELITY_ARGUMENT`), because the shared parsers name the profile field
//!
//! ## Does not own
//! - The JSON wire argument structs and the `*_from_json` handlers that call
//!   this resolution (`surface`)
//! - The bank read-policy seam itself, `ReadPolicyResolver` (`surface`)
//! - The floor ladder, the channel set, and the profile fields
//!   (`clio-config` `retention_levels` and `retention`)
//!
//! ## Boundary
//!
//! Changes to how a bank's read-side request fields resolve from a call's
//! arguments should normally require changes here. Changes to the wire argument
//! structs or to handler wiring should not require changes here.
//!
//! Keep this module focused on read-side request-field resolution.

#[cfg(test)]
#[path = "surface_policy_tests.rs"]
mod surface_policy_tests;

use crate::surface::ReadPolicyResolver;
use clio_config::retention::{DuplicateTolerance, RecallScope, RetentionProfile};
use clio_config::retention_levels::{PackFidelityDefault, ProvenanceFloorDefault};
use clio_types::{AmError, ErrorCode, TimeAxis};

/// The profile field name the ladder parser puts in its own message, and the
/// wire argument this surface receives instead.
const PROFILE_FLOOR_FIELD: &str = "provenance_floor_default";
/// The `provenance_floor` argument, as the schema names it.
const FLOOR_ARGUMENT: &str = "provenance_floor";
/// The profile field name the channel parser puts in its own message, and the
/// wire argument this surface receives instead.
const PROFILE_FIDELITY_FIELD: &str = "pack_fidelity_default";
/// The `fidelity` argument, as the schema names it.
const FIDELITY_ARGUMENT: &str = "fidelity";

/// The read-side fields one profile resolves to: scope, read-time tolerance,
/// curve state, provenance floor, and the pack fidelity channel.
pub(crate) type ResolvedReadPolicy = (
    RecallScope,
    DuplicateTolerance,
    bool,
    f64,
    Option<ProvenanceFloorDefault>,
    PackFidelityDefault,
);

/// Resolve the call's scope and the bank's read-side request fields from one
/// retention profile: an explicit `recall_scope`, `provenance_floor`, or
/// `fidelity` always wins, otherwise the profile's defaults apply. Without a
/// resolver the builtin default profile applies, whose provenance default is
/// `None` and whose fidelity default is `flow`. `compose_context` passes `None`
/// for the floor: it takes its value from the bank default only.
pub(crate) fn resolve_policy(
    raw: Option<&str>,
    raw_floor: Option<&str>,
    raw_fidelity: Option<&str>,
    bank: &str,
    policy: Option<&dyn ReadPolicyResolver>,
) -> Result<ResolvedReadPolicy, AmError> {
    let profile = match policy {
        Some(resolver) => resolver.read_policy(bank),
        None => RetentionProfile::default(),
    };
    let scope = match raw {
        None | Some("") => profile.recall_scope_default,
        Some(value) => RecallScope::parse(value)?,
    };
    // Absent means "no floor"; anything present must name a rung, including the
    // empty string, because an unparsed floor would widen the page to every
    // class while the caller believed it had narrowed it.
    let provenance_floor = match raw_floor {
        None => profile.provenance_floor_default,
        Some(value) => Some(ProvenanceFloorDefault::parse(value).map_err(|e| {
            // The ladder parser names the profile field it reads; this call site
            // received the wire argument, so only the leading field name is
            // remapped. The refusal test pins the wording this relies on.
            AmError::new(
                e.code,
                e.message.replacen(PROFILE_FLOOR_FIELD, FLOOR_ARGUMENT, 1),
            )
        })?),
    };
    // An absent channel means the bank default; a present one, the empty string
    // included, must name a channel, so a mistyped name is refused, not packed.
    let fidelity = match raw_fidelity {
        None => profile.pack_fidelity_default,
        // The channel parser names the profile field it reads, exactly as the
        // ladder parser does for the floor above; this call site received the
        // wire argument, so only the leading field name is remapped. The refusal
        // test pins the wording this relies on.
        Some(value) => PackFidelityDefault::parse(value).map_err(|e| {
            AmError::new(
                e.code,
                e.message
                    .replacen(PROFILE_FIDELITY_FIELD, FIDELITY_ARGUMENT, 1),
            )
        })?,
    };
    Ok((
        scope,
        profile.duplicate_tolerance_read,
        profile.retention_curve,
        profile.retention_floor,
        provenance_floor,
        fidelity,
    ))
}

pub(crate) fn parse_axis(raw: Option<&str>) -> Result<TimeAxis, AmError> {
    match raw {
        None | Some("") => Ok(TimeAxis::Valid),
        Some(value) => TimeAxis::parse(value),
    }
}

pub(crate) fn invalid(message: String) -> AmError {
    AmError::new(ErrorCode::InvalidArgument, message)
}
