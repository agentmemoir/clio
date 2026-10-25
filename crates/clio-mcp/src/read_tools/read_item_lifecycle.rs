// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for stamping an item's stored lifecycle state
//! onto a read payload as two additive top-level booleans.
//!
//! ## Owns
//! - `attach`, the two store probes plus the two payload keys they fill
//! - `DISCARDED_KEY` and `ARCHIVED_KEY`, the response keys the flags ride under
//!
//! ## Does not own
//! - Which read surfaces carry the flags (`read_tools`)
//! - The item projection or its SQL (`clio-store`)
//! - The discard and archive writes that set the markers (`clio-store`, `clio-hygiene`)
//!
//! ## Boundary
//!
//! Changes to how a read payload states an item's lifecycle should normally
//! require changes here. Changes to the stored marker columns or to a read
//! surface's own payload shape should not require changes here.
//!
//! Keep this module focused on reporting two markers, not on writing them.

use clio_store::Store;
use clio_types::AmError;
use serde_json::Value;

/// The key an item read carries its operations-removal marker under.
pub(crate) const DISCARDED_KEY: &str = "discarded";

/// The key an item read carries its hygiene archive marker under.
pub(crate) const ARCHIVED_KEY: &str = "archived";

/// Insert `discarded` and `archived` into `payload` from the stored row.
///
/// The identity read excludes a discarded row, so `None` here (with the item
/// present) is the discard marker. A failed probe is an error, never a `false`:
/// a read that could not establish the state must not report the item as live.
///
/// # Errors
/// Storage failures reading either marker.
pub(crate) fn attach(
    store: &dyn Store,
    bank_id: &str,
    item_id: &str,
    payload: &mut Value,
) -> Result<(), AmError> {
    let discarded = store.get_item_identity(bank_id, item_id)?.is_none();
    let archived = store.item_is_archived(bank_id, item_id)?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(DISCARDED_KEY.into(), Value::from(discarded));
        object.insert(ARCHIVED_KEY.into(), Value::from(archived));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../read_item_lifecycle_tests.rs"]
mod tests;
