// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the `admit --file` batch preview's
//! human text view, and specifically the noun it attaches to the count.
//!
//! ## Owns
//! - The singular/plural choice on the batch preview's count line
//!
//! ## Does not own
//! - The single-item `admit` preview and the other text views
//!   (`cli_write_core_tests`)
//! - The noun rule itself, and the restore and hygiene count lines that share
//!   it (`count_noun`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use serde_json::json;

use super::render_batch_preview;

/// Regression cover for issue #87: the batch preview line interpolated a bare
/// `u64` next to a hardcoded plural noun, so a one-item batch read
/// `preview: 1 items` while every other count line in the product read
/// `1 item`. Only the count of one changes; zero and two keep the plural.
#[test]
fn a_batch_preview_of_one_item_reads_singular() {
    let preview = |count: u64| {
        render_batch_preview(&json!({
            "count": count,
            "summary": {"would_admit": count, "would_reject": 0},
            "writes": 0
        }))
    };
    assert_eq!(
        preview(1),
        "preview: 1 item (would_admit: 1, would_reject: 0, writes: 0)",
        "a one-item batch must read as one item"
    );
    assert_eq!(
        preview(2),
        "preview: 2 items (would_admit: 2, would_reject: 0, writes: 0)",
        "two items keep the plural"
    );
    assert_eq!(
        preview(0),
        "preview: 0 items (would_admit: 0, would_reject: 0, writes: 0)",
        "zero takes the plural, which is what an operator expects to read"
    );
}
