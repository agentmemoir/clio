// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the internal file-export writer of local
//! event review output: the export availability gate, the content-free
//! rendering of the review rows, and the file write.
//!
//! ## Owns
//! - `write_review_export` and the content-free review text rendering
//!
//! ## Does not own
//! - The review query and its actor gate (see `local_review.rs`)
//! - The durable local event writer (`clio-store` owns it)
//! - Public exposure: the crate root re-exports no symbol from here
//!
//! ## Boundary
//!
//! Changes to how a review file export is gated, rendered, or written should
//! normally require changes here. Changes to the review query should not
//! require changes here.
//!
//! Keep this module focused on the internal review file-export writer.

use crate::local_review::{LocalReviewQuery, authorize, review_local_events};
use crate::subject_request_events::BindingControls;
use clio_store::{LocalEventState, LocalEventStore, LocalEventView};
use clio_types::{AmError, ErrorCode, OpContext};
use std::fmt::Write as _;
use std::path::Path;

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

/// Write a content-free review export to `destination`.
///
/// This writer is internal: it is `pub(crate)` and the crate root re-exports no
/// symbol from this module, so there is no public route to it. It refuses for
/// every scope until the copy controls are accepted, and the file export stays
/// unavailable until durable destination custody is merged.
#[allow(dead_code)] // intentionally unwired until durable destination custody is merged.
pub(crate) fn write_review_export(
    ctx: &OpContext,
    store: &dyn LocalEventStore,
    query: &LocalReviewQuery,
    controls: BindingControls,
    destination: &Path,
) -> Result<(), AmError> {
    authorize(ctx.actor)?;
    if !controls.copy_controls_accepted {
        return Err(AmError::new(
            ErrorCode::Forbidden,
            "review file export is unavailable until durable destination custody is merged",
        ));
    }
    let page = review_local_events(ctx, store, query)?;
    std::fs::write(destination, render_rows(&page.rows)).map_err(|error| {
        AmError::new(
            ErrorCode::Internal,
            format!("review export write failed: {error}"),
        )
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "local_review_export_tests.rs"]
mod local_review_export_tests;
