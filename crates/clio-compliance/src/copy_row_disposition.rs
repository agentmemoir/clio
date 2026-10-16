// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the parse of one stored copy-inventory row
//! into the copy-disposition vocabulary: the row's custody, owner, required
//! action, and technical state, and the fail-closed mapping of each.
//!
//! ## Owns
//! - `copy_row_disposition`, the stored copy row's disposition
//! - `custody_class` and `copy_state`, the fail-closed name mappings
//!
//! ## Does not own
//! - The disposition vocabulary itself and the fold that consumes it
//!   (`copy_disposition.rs`)
//! - The durable copy-inventory SQL and its row contract (`clio-store`)
//! - Folding a set of dispositions into the closure-gate facts
//!   (`subject_request_store_wiring.rs`)
//!
//! ## Boundary
//!
//! Changes to how a stored copy row is read into the vocabulary should
//! normally require changes here. Changes to the vocabulary or to the closure
//! folds should not.
//!
//! Keep this module focused on parsing one stored copy row.

use crate::copy_disposition::{
    CopyAction, CopyCustodyClass, CopyDisposition, CopyOwner, CopyRequiredAction, CopyState,
};
use crate::subject_request::RequestActionState;
use clio_store::CopyInventoryRow;
use clio_types::{AmError, ErrorCode};

/// The stored custody class of a copy row.
///
/// A class name the disposition vocabulary does not hold is
/// [`CopyCustodyClass::Unknown`], which refuses any completion claim.
fn custody_class(text: &str) -> CopyCustodyClass {
    match text {
        "clio" => CopyCustodyClass::Clio,
        "operator" => CopyCustodyClass::Operator,
        "caller" => CopyCustodyClass::Caller,
        "peer" => CopyCustodyClass::Peer,
        "review_output" => CopyCustodyClass::ReviewOutput,
        _ => CopyCustodyClass::Unknown,
    }
}

/// The technical erasure state of a copy row.
///
/// A state name the disposition vocabulary does not hold is
/// [`CopyState::Unknown`], which refuses a technical-completion claim.
fn copy_state(text: &str) -> CopyState {
    match text {
        "pending" => CopyState::Pending,
        "done" => CopyState::Done,
        "failed" => CopyState::Failed,
        _ => CopyState::Unknown,
    }
}

/// Parse one stored copy row into its disposition.
///
/// The row is metadata only: the disposition carries the row's custody,
/// owner, required action, and state, and no copy content. The parse fails
/// closed: an owner or a required action the vocabulary does not hold
/// refuses the row, because neither has an unknown variant, while an
/// unrecognized custody or state maps to its own unknown variant.
///
/// The row's action stays open while the copy owes a durable result: a named
/// exception reason or any technical state other than done keeps it open, so
/// the row attaches to a named open gap instead of a request result.
///
/// # Errors
/// An owner or a required action the disposition vocabulary does not hold.
pub fn copy_row_disposition(row: &CopyInventoryRow) -> Result<CopyDisposition, AmError> {
    let owner = match row.owner.as_str() {
        "harness" => CopyOwner::Harness,
        "system" => CopyOwner::System,
        other => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "copy `{}` names owner `{other}`, which carries no compliance privilege",
                    row.copy_id
                ),
            ));
        }
    };
    let kind = match row.required_action.as_str() {
        "erase" => CopyRequiredAction::Erase,
        "remove" => CopyRequiredAction::Remove,
        "delivery" => CopyRequiredAction::Delivery,
        "notice" => CopyRequiredAction::Notice,
        other => {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!(
                    "copy `{}` names required action `{other}`, which the vocabulary does not hold",
                    row.copy_id
                ),
            ));
        }
    };
    let state = copy_state(&row.state);
    let open = row.exception_reason.is_some() || state != CopyState::Done;
    Ok(CopyDisposition {
        copy_id: row.copy_id.clone(),
        custody: custody_class(&row.custody),
        owner,
        state,
        actions: vec![CopyAction {
            kind,
            state: if open {
                RequestActionState::Open
            } else {
                RequestActionState::Closed
            },
        }],
    })
}
