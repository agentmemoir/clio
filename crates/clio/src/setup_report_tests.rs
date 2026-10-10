// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the change model and the preview
//! layout: what counts as new, unchanged, or removed, and the order the lines
//! are printed in.
//!
//! ## Owns
//! - Change-model behavior under test for `setup_report`
//!
//! ## Does not own
//! - What the preview contains (see `setup_preview_tests`)
//! - The amend and `switch` prompts (see `setup_report_amend_tests`)
//!
//! ## Boundary
//!
//! Changes to the change model or to the block layout should normally update
//! tests here. Changes to what a run plans should not require changes here.
//!
//! Keep this module focused on the model and the layout.

use super::*;
use crate::cli_style::Palette;
use serde_json::json;

/// A lookup that answers from a document, standing in for the runtime's
/// "what does the file set" question.
fn from(document: &Value) -> impl Fn(&str) -> Option<Value> + '_ {
    move |path| value_at(document, path)
}

/// The identity display, so a model assertion is not about masking.
fn plain(path: &str, value: &Value) -> String {
    let _ = path;
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToOwned::to_owned)
}

/// One planned entry paired with its name and value.
fn entry(path: &str, value: &str) -> (String, Value) {
    (path.to_owned(), Value::String(value.to_owned()))
}

#[test]
fn a_state_prints_the_word_the_plan_uses() {
    assert_eq!(ChangeState::New.as_str(), "new");
    assert_eq!(ChangeState::Unchanged.as_str(), "unchanged");
    assert_eq!(ChangeState::Removed.as_str(), "removed");
}

#[test]
fn a_planned_entry_the_config_does_not_hold_is_new() {
    let planned = vec![entry("embed.url", "https://a.example.test")];
    let changes = changes(&planned, &[], &from(&json!({})), &plain);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].state, ChangeState::New);
}

#[test]
fn a_planned_entry_the_config_already_holds_is_unchanged() {
    let planned = vec![entry("embed.url", "https://a.example.test")];
    let current = json!({ "embed": { "url": "https://a.example.test" } });
    let changes = changes(&planned, &[], &from(&current), &plain);
    assert_eq!(changes[0].state, ChangeState::Unchanged);
}

#[test]
fn a_watched_path_the_config_holds_and_the_run_drops_is_removed() {
    let planned = vec![entry("embed.url", "https://a.example.test")];
    let current = json!({ "credentials": { "embed_api_key": "sk-old" } });
    let changes = changes(
        &planned,
        &["credentials.embed_api_key"],
        &from(&current),
        &plain,
    );
    let removed = changes
        .iter()
        .find(|change| change.path.contains("api key"))
        .expect("the dropped key is shown");
    assert_eq!(removed.state, ChangeState::Removed);
    assert_eq!(removed.note.as_deref(), Some("will be removed"));
}

#[test]
fn a_watched_path_the_config_never_held_is_not_shown_at_all() {
    // Showing a removal for something that was never there would read as a
    // warning about a field the operator has never set.
    let changes = changes(
        &[],
        &["credentials.embed_api_key"],
        &from(&json!({})),
        &plain,
    );
    assert!(changes.is_empty(), "{changes:?}");
}

#[test]
fn a_watched_path_the_run_also_writes_is_not_shown_twice() {
    let planned = vec![entry("embed.url", "https://a.example.test")];
    let current = json!({ "embed": { "url": "https://old.example.test" } });
    let changes = changes(&planned, &["embed.url"], &from(&current), &plain);
    assert_eq!(changes.len(), 1, "{changes:?}");
}

#[test]
fn a_dotted_path_is_named_the_way_the_preview_prints_it() {
    assert_eq!(display_name("backend"), "Database backend");
    assert_eq!(display_name("store.database_url"), "Database address");
    assert_eq!(display_name("store.sqlite_path"), "SQLite file");
    assert_eq!(display_name("embed.provider"), "embed provider");
    assert_eq!(
        display_name("credentials.embed_api_key"),
        "credentials embed api key"
    );
}

#[test]
fn the_block_names_come_out_in_the_plans_order() {
    let view = PreviewView {
        blocks: vec![
            Block {
                name: "Database",
                changes: vec![Change {
                    path: "Database backend".to_owned(),
                    value: "postgres".to_owned(),
                    state: ChangeState::New,
                    note: None,
                }],
            },
            Block {
                name: "Embedding",
                changes: vec![Change {
                    path: "embed model".to_owned(),
                    value: "bge".to_owned(),
                    state: ChangeState::Unchanged,
                    note: None,
                }],
            },
        ],
        will_start: vec!["postgres"],
        will_remove: vec!["rerank model".to_owned()],
        will_back_up: None,
        tested: vec![Tested {
            what: "database",
            detail: "connected".to_owned(),
            was_tested: true,
        }],
    };
    let mut out: Vec<u8> = Vec::new();
    preview(&mut out, Palette::new(false), &view).expect("a Vec");
    let printed = String::from_utf8(out).expect("utf8");
    let database = printed.find("Database").expect("the database block");
    let embedding = printed.find("embed model").expect("the embedding block");
    assert!(database < embedding, "{printed}");
    assert!(printed.contains("Will start    postgres"), "{printed}");
    assert!(printed.contains("Will remove   rerank model"), "{printed}");
    assert!(
        printed.contains("Will back up  nothing, there is no config file yet"),
        "{printed}"
    );
    assert!(printed.contains("Tested"), "{printed}");
}

#[test]
fn an_untested_line_is_visibly_different_from_a_tested_one() {
    let view = PreviewView {
        blocks: Vec::new(),
        will_start: Vec::new(),
        will_remove: Vec::new(),
        will_back_up: Some("/tmp/deployment.json".to_owned()),
        tested: vec![
            Tested {
                what: "database",
                detail: "connected".to_owned(),
                was_tested: true,
            },
            Tested {
                what: "embedding",
                detail: "the local container was not started yet".to_owned(),
                was_tested: false,
            },
        ],
    };
    let mut out: Vec<u8> = Vec::new();
    preview(&mut out, Palette::new(false), &view).expect("a Vec");
    let printed = String::from_utf8(out).expect("utf8");
    assert!(printed.contains("connected"), "{printed}");
    assert!(
        printed.contains("not tested (the local container was not started yet)"),
        "{printed}"
    );
    assert!(
        printed.contains("Will back up  /tmp/deployment.json"),
        "{printed}"
    );
}

#[test]
fn nothing_to_start_or_remove_says_nothing_rather_than_an_empty_cell() {
    let view = PreviewView {
        blocks: Vec::new(),
        will_start: Vec::new(),
        will_remove: Vec::new(),
        will_back_up: None,
        tested: Vec::new(),
    };
    let mut out: Vec<u8> = Vec::new();
    preview(&mut out, Palette::new(false), &view).expect("a Vec");
    let printed = String::from_utf8(out).expect("utf8");
    assert!(printed.contains("Will start    nothing"), "{printed}");
    assert!(printed.contains("Will remove   nothing"), "{printed}");
    assert!(!printed.contains("Tested"), "{printed}");
}

#[test]
fn a_second_column_is_printed_after_the_state() {
    let view = PreviewView {
        blocks: vec![Block {
            name: "Database",
            changes: vec![Change {
                path: "Database password".to_owned(),
                value: "(24 characters)".to_owned(),
                state: ChangeState::New,
                note: Some("not shown".to_owned()),
            }],
        }],
        will_start: Vec::new(),
        will_remove: Vec::new(),
        will_back_up: None,
        tested: Vec::new(),
    };
    let mut out: Vec<u8> = Vec::new();
    preview(&mut out, Palette::new(false), &view).expect("a Vec");
    let printed = String::from_utf8(out).expect("utf8");
    let state = printed.find("new").expect("the state");
    let note = printed.find("not shown").expect("the note");
    assert!(state < note, "{printed}");
    assert!(printed.contains("(24 characters)"), "{printed}");
}
