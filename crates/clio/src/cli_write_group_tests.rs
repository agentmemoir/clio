// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the refusal mapping for a batch preview: a preview in which
//! every item failed validation is refused, and a preview with one valid item is not.
//!
//! ## Owns
//! - Assertions over `batch_preview_refusal`
//!
//! ## Does not own
//! - The admission decision itself (see `clio-admission`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::batch_preview_refusal;
use serde_json::json;

#[test]
fn a_batch_where_every_item_failed_validation_is_refused_with_the_first_index() {
    let refusal = batch_preview_refusal(&json!({
        "items": [
            {"pass": false, "error": {"code": "invalid_argument", "message": "epistemic_kind is required"}},
            {"pass": false, "error": {"code": "invalid_argument", "message": "category is required"}},
        ]
    }))
    .expect("an all-invalid batch is a refusal");
    assert_eq!(refusal.code, "invalid_argument");
    assert_eq!(
        refusal.message,
        "no item previewed: all 2 items failed validation; item 0: epistemic_kind is required"
    );
}

#[test]
fn a_batch_with_one_item_that_passed_validation_is_not_refused() {
    let payload = json!({
        "items": [
            {"pass": false, "error": {"code": "invalid_argument", "message": "category is required"}},
            {"pass": false, "rejection_reason": "novelty below floor"},
        ]
    });
    assert!(batch_preview_refusal(&payload).is_none());
}

#[test]
fn an_empty_batch_is_not_refused_here() {
    assert!(batch_preview_refusal(&json!({"items": []})).is_none());
}
