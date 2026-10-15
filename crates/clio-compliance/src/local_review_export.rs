// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the internal file-export writer of local
//! event review output: the content-free rendering of the review rows and the
//! durable destination-custody delivery of that rendering.
//!
//! ## Owns
//! - `write_review_export` and the content-free review text rendering
//!
//! ## Does not own
//! - The review query and its actor gate (see `local_review.rs`)
//! - The delivery sequence and destination custody (`export_custody.rs` owns it)
//! - The durable local event writer (`clio-store` owns it)
//!
//! ## Boundary
//!
//! Changes to how a review file export is gated, rendered, or delivered should
//! normally require changes here. Changes to the review query should not
//! require changes here.
//!
//! Keep this module focused on the internal review file-export writer.

use crate::export_custody::{DeliveryRefusal, DeliveryResult, DeliverySpec, deliver_export};
use crate::local_review::{LocalReviewQuery, authorize, review_local_events};
use crate::subject_request_events::BindingControls;
use clio_store::{DestinationCustodyStore, LocalEventState, LocalEventStore, LocalEventView};
use clio_types::{AmError, ErrorCode, OpContext};
use std::fmt::Write as _;

/// The stable label rendered for a row's state.
fn state_label(state: LocalEventState) -> &'static str {
    match state {
        LocalEventState::Pending => "pending",
        LocalEventState::Allowed => "allowed",
        LocalEventState::Denied => "denied",
    }
}

/// Render the ordered rows to the content-free export text: one line per row
/// with its sequence position, action id, and state. The outcome result text is
/// omitted, so a copy carries no subject content.
fn render_rows(rows: &[LocalEventView]) -> String {
    let mut text = String::new();
    for row in rows {
        let _ = writeln!(
            text,
            "{}\t{}\t{}",
            row.seq,
            row.action_id,
            state_label(row.state)
        );
    }
    text
}

/// Write a content-free review export to `spec.destination`.
///
/// Only a harness or system actor may export. The rows are read through the
/// caller's event store, rendered content-free, and delivered through the
/// durable destination-custody boundary, which records the event intent and
/// the pending custody row before any file effect and refuses to overwrite an
/// existing caller file.
///
/// # Errors
/// A forbidden actor or an unaccepted copy binding is `Forbidden`; a duplicate
/// or non-regular destination and a failed durable intent are
/// `InvalidArgument`; any other delivery failure is `Internal`.
pub fn write_review_export(
    ctx: &OpContext,
    events: &dyn LocalEventStore,
    custody: &dyn DestinationCustodyStore,
    query: &LocalReviewQuery,
    controls: &BindingControls,
    spec: &DeliverySpec<'_>,
) -> Result<(), AmError> {
    authorize(ctx.actor)?;
    let page = review_local_events(ctx, events, query)?;
    let bytes = render_rows(&page.rows);
    match deliver_export(ctx, events, custody, controls, spec, bytes.as_bytes()) {
        DeliveryResult::Delivered => Ok(()),
        DeliveryResult::Refused(DeliveryRefusal::BindingNotAccepted) => Err(AmError::new(
            ErrorCode::Forbidden,
            "review file export is refused until the copy controls are accepted",
        )),
        DeliveryResult::Refused(
            DeliveryRefusal::DestinationExists
            | DeliveryRefusal::DestinationNotRegularFile
            | DeliveryRefusal::DurableIntentFailed,
        ) => Err(AmError::new(
            ErrorCode::InvalidArgument,
            "review file export is refused: destination custody is not available",
        )),
        DeliveryResult::Failed(reason) => Err(AmError::new(
            ErrorCode::Internal,
            format!("review export delivery failed: {reason}"),
        )),
    }
}

#[cfg(test)]
#[path = "local_review_export_tests.rs"]
mod local_review_export_tests;

#[cfg(test)]
#[path = "review_export_scenarios_tests.rs"]
pub(crate) mod review_export_scenarios_tests;

#[cfg(test)]
#[path = "review_export_scenarios_pg_tests.rs"]
mod review_export_scenarios_pg_tests;
