// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the privileged local event review: a bounded,
//! ordered keyset read of the durable local event rows, the inventory of the
//! review output routes, and the custody record of one captured copy.
//!
//! ## Owns
//! - `LocalReviewQuery`, `LocalReviewPage`, and `review_local_events`
//! - `ReviewOutputRoute`, `review_output_routes`, and
//!   `record_review_output_custody`
//! - The privileged actor gate shared with the internal file-export writer
//!
//! ## Does not own
//! - The durable local event writer and its SQL (`clio-store` owns those)
//! - The internal file-export writer (see `local_review_export.rs`)
//! - Copy-custody field validation (see `subject_request_manifest.rs`)
//! - The public-binding gate for a file export (see `subject_request_events.rs`)
//!
//! ## Boundary
//!
//! Changes to the review query shape, the page cursor, the output routes, or
//! the custody call should normally require changes here. Changes to the local
//! event store or to the copy-custody field contract should not require changes
//! here.
//!
//! Keep this module focused on the local event review surface.

use crate::bundle::sha256_hex_bytes;
use crate::subject_request_manifest::{CopyCustodyManifest, DIGEST_ALGORITHM_PREFIX};
use clio_store::{LocalEventStore, LocalEventView};
use clio_types::{Actor, AmError, ErrorCode, OpContext};

/// Producer identity recorded on every review-output custody manifest.
pub const REVIEW_OUTPUT_PRODUCER: &str = "clio";

/// The review page size a caller gets when it does not choose one.
pub const DEFAULT_REVIEW_LIMIT: usize = 100;

/// The largest review page size a caller may ask for.
pub const MAX_REVIEW_LIMIT: usize = 1000;

/// A bounded, keyset-paged request for local event review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalReviewQuery {
    /// Maximum rows to return; must be between 1 and [`MAX_REVIEW_LIMIT`].
    pub limit: usize,
    /// Resume after this decimal sequence position; `None` starts at the first.
    pub cursor: Option<String>,
}

/// One page of ordered local event rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalReviewPage {
    /// The rows of this page, in ascending sequence position.
    pub rows: Vec<LocalEventView>,
    /// The sequence position to resume from when a next page exists.
    pub next_cursor: Option<String>,
    /// Whether rows remain after this page.
    pub has_more: bool,
}

/// A local review output route and whether it is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOutputRoute {
    /// Captured content-free standard output; the operator copy.
    OperatorStdout,
    /// A file export under durable destination custody.
    FileExport,
}

impl ReviewOutputRoute {
    /// Whether this route may be used today.
    #[must_use]
    pub const fn available(self) -> bool {
        matches!(self, Self::OperatorStdout | Self::FileExport)
    }
}

/// Every local review output route, for inventory.
#[must_use]
pub fn review_output_routes() -> [ReviewOutputRoute; 2] {
    [
        ReviewOutputRoute::OperatorStdout,
        ReviewOutputRoute::FileExport,
    ]
}

/// Only harness/system actors carry local review privilege.
pub(crate) fn authorize(actor: Actor) -> Result<(), AmError> {
    match actor {
        Actor::Harness | Actor::System => Ok(()),
        Actor::Agent | Actor::User => Err(AmError::new(
            ErrorCode::Forbidden,
            "local event review requires a harness or system actor with compliance privilege",
        )),
    }
}

/// Parse the keyset cursor: `None` starts before the first row, and a present
/// cursor must be a non-negative decimal sequence number.
fn parse_cursor(cursor: Option<&str>) -> Result<i64, AmError> {
    let Some(text) = cursor else {
        return Ok(0);
    };
    let value = text.parse::<i64>().map_err(|_| {
        AmError::new(
            ErrorCode::InvalidArgument,
            "review cursor must be a decimal sequence number",
        )
    })?;
    if value < 0 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "review cursor must not be negative",
        ));
    }
    Ok(value)
}

/// Read one ordered, bounded page of local event rows.
///
/// Only a harness or system actor may review. The list is read in full and
/// filtered in memory because the store trait offers no paged read and its key
/// is a store-assigned gap-free sequence. A row whose outcome is still pending
/// is a visible gap, and review never reconciles it. The rows are content-free:
/// the stored outcome `result` text is stripped, matching the file-export
/// writer, so a review page carries no subject content.
pub fn review_local_events(
    ctx: &OpContext,
    store: &dyn LocalEventStore,
    query: &LocalReviewQuery,
) -> Result<LocalReviewPage, AmError> {
    authorize(ctx.actor)?;
    if query.limit == 0 || query.limit > MAX_REVIEW_LIMIT {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("review limit must be between 1 and {MAX_REVIEW_LIMIT}"),
        ));
    }
    let cursor = parse_cursor(query.cursor.as_deref())?;
    let mut rows: Vec<LocalEventView> = store
        .list_events()?
        .into_iter()
        .filter(|row| row.seq > cursor)
        .map(|mut row| {
            row.result = None;
            row
        })
        .collect();
    let has_more = rows.len() > query.limit;
    rows.truncate(query.limit);
    let next_cursor = if has_more {
        rows.last().map(|row| row.seq.to_string())
    } else {
        None
    };
    Ok(LocalReviewPage {
        rows,
        next_cursor,
        has_more,
    })
}

/// Record the custody of one captured operator copy of review output.
///
/// Only a harness or system actor may record the copy. The digest is over the
/// caller-supplied copy bytes, so the caller decides what the copy holds.
pub fn record_review_output_custody(
    ctx: &OpContext,
    content: &[u8],
    receiver: &str,
    produced_at: &str,
    received_at: &str,
) -> Result<CopyCustodyManifest, AmError> {
    authorize(ctx.actor)?;
    let copy_digest = format!("{DIGEST_ALGORITHM_PREFIX}{}", sha256_hex_bytes(content));
    CopyCustodyManifest::write(
        REVIEW_OUTPUT_PRODUCER,
        receiver,
        &copy_digest,
        produced_at,
        received_at,
    )
}

#[cfg(test)]
#[path = "local_review_tests.rs"]
mod local_review_tests;
