// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for compiling a half-open bi-temporal item
//! window into one parameterized `SELECT` shared by both storage backends.
//!
//! ## Owns
//! - Window input validation, axis column selection, and placeholder numbering
//! - The candidate predicate, ordering, and limit clause text
//!
//! ## Does not own
//! - Statement execution or parameter binding (the backend window modules)
//! - Caller-side domain, archive, and `as_of` filtering of fetched ids
//!
//! ## Boundary
//!
//! Changes to window membership or ordering should normally require changes here.
//! Changes to one backend's execution should not require changes here.
//!
//! Keep this module focused on the shared item-window query compiler.

use clio_types::{AmError, ErrorCode, TimeAxis};

/// Upper bound on [`crate::Store::item_ids_in_window`] results.
///
/// A retrieval arm never needs more than its candidate fetch, and the bound
/// keeps a caller-supplied limit from turning one query into an unbounded scan.
pub const MAX_WINDOW_ITEMS: usize = 1000;

/// A compiled window statement plus the string parameters it references.
pub(crate) struct CompiledWindow {
    /// The full `SELECT`, including its `ORDER BY` and `LIMIT` clauses.
    pub sql: String,
    /// String parameter values in placeholder order: bank, end, start.
    pub params: Vec<String>,
}

/// Compile a window query for `placeholder` (`'?'` or `'$'`).
///
/// The window is half-open: an item matches when its interval start is before
/// `end` and its interval end is absent or after `start`. Bounds compare as
/// ISO-8601 text, the same convention the store writes.
///
/// # Errors
/// Empty bank or bounds, `start >= end`, a zero limit, or a limit above
/// [`MAX_WINDOW_ITEMS`].
pub(crate) fn compile_window(
    bank_id: &str,
    axis: TimeAxis,
    start: &str,
    end: &str,
    limit: usize,
    placeholder: char,
) -> Result<CompiledWindow, AmError> {
    if bank_id.is_empty() {
        return Err(AmError::new(
            ErrorCode::MissingBank,
            "item window requires a non-empty bank",
        ));
    }
    if start.is_empty() || end.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item window bounds must be non-empty",
        ));
    }
    if start >= end {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item window start must be before its end",
        ));
    }
    if limit == 0 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "item window limit must be positive",
        ));
    }
    if limit > MAX_WINDOW_ITEMS {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("item window limit is capped at {MAX_WINDOW_ITEMS}, got {limit}"),
        ));
    }
    let (start_col, end_col) = match axis {
        TimeAxis::Valid => ("valid_from", "valid_until"),
        TimeAxis::Transaction => ("tx_from", "tx_until"),
    };
    let sql = format!(
        "SELECT id FROM items WHERE bank_id = {placeholder}1 AND discarded_at IS NULL \
         AND archived_at IS NULL AND tx_until IS NULL AND {start_col} < {placeholder}2 \
         AND ({end_col} IS NULL OR {end_col} > {placeholder}3) \
         ORDER BY {start_col} DESC, id ASC LIMIT {placeholder}4"
    );
    Ok(CompiledWindow {
        sql,
        params: vec![bank_id.to_owned(), end.to_owned(), start.to_owned()],
    })
}
