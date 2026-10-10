// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving one bank retention profile's
//! read-side request controls from a call's wire arguments: the provenance floor
//! and the pack fidelity channel.
//!
//! ## Owns
//! - `read_provenance_floor`: an explicit `provenance_floor` argument first,
//!   the bank's `provenance_floor_default` second, and no floor when neither
//!   names one
//! - `read_fidelity`: an explicit `fidelity` argument first, the bank's
//!   `pack_fidelity_default` second, and `flow` when neither names one
//! - The wire-argument name each refusal is remapped to
//!   (`PROVENANCE_FLOOR_ARG` and `FIDELITY_ARG`), because the shared parsers
//!   name the profile field instead
//!
//! ## Does not own
//! - Which read tools publish these arguments (`read_retrieve` owns the
//!   handlers and their binding)
//! - The profile fields themselves and their stored defaults (`clio-config`
//!   `RetentionProfile`)
//! - The floor ladder and the channel semantics (`clio-retrieve` `floors` and
//!   `pack_forms`)
//!
//! ## Boundary
//!
//! Changes to how a bank read-side control resolves from its arguments should
//! normally require changes here. Changes to read-handler binding or to pack
//! assembly should not require changes here.
//!
//! Keep this module focused on read-side control resolution.

use crate::runtime::McpState;
use clio_config::retention_levels::{PackFidelityDefault, ProvenanceFloorDefault};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;

/// The wire argument this tool publishes for the floor. The shared parser's
/// refusal names the store field the same value is saved under, so the message
/// is remapped to this name at the call site that owns the wire name.
const PROVENANCE_FLOOR_ARG: &str = "provenance_floor";

/// Resolve the effective provenance floor, the single decision behind both read
/// paths: an explicit `provenance_floor` argument wins, otherwise the bank's
/// `provenance_floor_default` applies, and a bank that states none means no
/// floor. The bank default supplies a value and changes nothing else, so it
/// cannot widen or narrow a call that named its own floor.
///
/// `compose_context` passes no raw argument, so the pack takes the bank default
/// only and the injection path cannot bypass a policy the bank stated.
///
/// A present value must name a rung, and the ladder is closed, so the parse is
/// the gate: a mistyped or empty value is `invalid_argument` and returns no
/// page, never an unfiltered one. An absent key and an explicit `null` are the
/// same request, and both take the bank default.
pub(crate) fn read_provenance_floor(
    state: &McpState,
    bank: &str,
    raw: Option<&Value>,
) -> Result<Option<ProvenanceFloorDefault>, AmError> {
    match raw {
        None | Some(Value::Null) => Ok(state.retention_effective(bank).0.provenance_floor_default),
        // Only the leading field name is remapped, so the echoed value keeps the
        // text the caller sent, and the code is carried through unchanged.
        Some(Value::String(value)) => ProvenanceFloorDefault::parse(value).map(Some).map_err(|e| {
            AmError::new(
                e.code,
                e.message
                    .replacen("provenance_floor_default", PROVENANCE_FLOOR_ARG, 1),
            )
        }),
        Some(_) => Err(AmError::new(
            ErrorCode::InvalidArgument,
            "provenance_floor must be a string naming verified|stated|third_party|inferred",
        )),
    }
}

/// The wire argument `compose_context` publishes for the channel. The shared
/// parser's refusal names the profile field the same value is saved under, so
/// the message is remapped to this name at the call site that owns the wire
/// name.
const FIDELITY_ARG: &str = "fidelity";

/// Resolve the effective pack fidelity channel, the single decision behind
/// both pack paths: an explicit `fidelity` argument wins, otherwise the bank's
/// `pack_fidelity_default` applies, and a bank that states none packs `flow`.
/// The bank default supplies a value and changes nothing else, so it cannot
/// widen or narrow a call that named its own channel.
///
/// An absent key and an explicit `null` are the same request, and both take the
/// bank default. A present value must name a channel, and the set is closed, so
/// the parse is the gate: a mistyped value is `invalid_argument` and returns no
/// pack, never a downgraded one.
pub(crate) fn read_fidelity(
    state: &McpState,
    bank: &str,
    raw: Option<&Value>,
) -> Result<PackFidelityDefault, AmError> {
    match raw {
        None | Some(Value::Null) => Ok(state.retention_effective(bank).0.pack_fidelity_default),
        // Only the leading field name is remapped, so the echoed value keeps the
        // text the caller sent, and the code is carried through unchanged.
        Some(Value::String(value)) => PackFidelityDefault::parse(value).map_err(|e| {
            AmError::new(
                e.code,
                e.message.replacen("pack_fidelity_default", FIDELITY_ARG, 1),
            )
        }),
        Some(_) => Err(AmError::new(
            ErrorCode::InvalidArgument,
            "fidelity must be a string naming flow|exact",
        )),
    }
}

#[cfg(test)]
#[path = "read_read_policy_tests.rs"]
mod read_read_policy_tests;
