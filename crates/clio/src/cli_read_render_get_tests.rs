// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing that the `get`/`show` text view
//! states an item's lifecycle marker, since a discarded item still reads by id
//! and otherwise reads as live to anyone not asking for JSON.
//!
//! ## Owns
//! - The `discarded` and `archived` lines appearing only on a marked item
//! - A live item's text view gaining no line
//!
//! ## Does not own
//! - Where the two flags come from (`clio-mcp`'s `get_item`, which reads them)
//! - The rest of the get text view (`cli_read_render::render_get`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_render::render_text;
use crate::cli_style::Palette;
use serde_json::{Value, json};

fn render_get(payload: &Value) -> String {
    render_text("get", payload, Palette::new(false))
}

/// The text view's own fields, in print order, for one lifecycle marking.
const UNMARKED_VIEW: &str =
    "id: itm-1\ncategory: persona\nepistemic_kind: fact\ngist: User prefers dark mode";

/// The payload shape `get_item` returns, minus the fields this view does not
/// print. The lifecycle flags are additive top-level keys.
fn item(discarded: bool, archived: bool) -> Value {
    json!({
        "item_id": "itm-1",
        "category": "persona",
        "epistemic_kind": "fact",
        "gist": "User prefers dark mode",
        "discarded": discarded,
        "archived": archived
    })
}

#[test]
fn a_discarded_item_says_it_is_discarded() {
    let text = render_get(&item(true, false));
    assert_eq!(text, format!("{UNMARKED_VIEW}\ndiscarded: true"));
}

/// The archive marker is the other lifecycle flag the payload carries, and it
/// gets the same treatment.
#[test]
fn an_archived_item_says_it_is_archived() {
    let text = render_get(&item(false, true));
    assert_eq!(text, format!("{UNMARKED_VIEW}\narchived: true"));
}

/// A live item's view must not change at all: the new lines are additive only
/// where a marker is set, so no blank lines and no `discarded: false`.
#[test]
fn a_live_item_gains_no_lifecycle_line() {
    assert_eq!(render_get(&item(false, false)), UNMARKED_VIEW);
}
