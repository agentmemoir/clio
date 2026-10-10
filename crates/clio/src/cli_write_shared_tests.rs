// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` shared-surface commands: shared
//! store/retrieve/discard (including the Forbidden gate and explicit-actor
//! requirement) and the text/JSON render views for them.
//!
//! ## Owns
//! - Behavior and golden tests for `cli_write_shared`
//!
//! ## Does not own
//! - The commands under test (`cli_write_shared`)
//! - Reviewer semantics for `validate` (see `cli_write_validate_tests`)
//! - Shared-bank routing semantics (see `clio-mcp` / `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{json_of, state};
use crate::cli_write_test_support::{run_write_resolved, run_write_state, shared_state};
use serde_json::json;
use std::process::ExitCode;

#[test]
fn shared_store_then_retrieve_round_trip_on_the_shared_bank() {
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared store",
        &[
            "shared convention: prefer terse answers",
            "--epistemic-kind",
            "fact",
            "--category",
            "tool_config",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["pass"], json!(true), "{payload}");
    let id = payload["id"].as_str().expect("stored id").to_owned();
    // The store routes exclusively to the dedicated shared bank.
    assert!(
        s.store.get_memory_item("shared", &id).unwrap().is_some(),
        "item must live in the shared bank"
    );
    let (code, out, err) = run_write_state(
        "shared retrieve",
        &["shared convention", "--actor", "agent", "--shared-bank"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    let hits = payload["hits"].as_array().cloned().unwrap_or_default();
    assert_eq!(hits.len(), 1, "{payload}");
    assert_eq!(hits[0]["item_id"], json!(id), "{payload}");
}

#[test]
fn shared_store_without_the_enabled_surface_exits_one_forbidden() {
    // The `--shared-bank` CLI flag only opts the deployment in at open time;
    // state opened without it gates the tool exactly as the MCP path does.
    let s = state();
    let (code, out, err) = run_write_state(
        "shared store",
        &[
            "shared convention",
            "--epistemic-kind",
            "fact",
            "--category",
            "tool_config",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("shared bank surface"), "{err}");
    assert_eq!(
        s.store
            .count_items(&clio_store::ItemInspectFilter::new("shared"))
            .unwrap(),
        0,
        "the gated store must write nothing"
    );
}

#[test]
fn shared_store_requires_epistemic_kind() {
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared store",
        &["shared convention", "--actor", "agent", "--shared-bank"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--epistemic-kind"), "{err}");
}

#[test]
fn shared_store_maps_every_optional_flag() {
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared store",
        &[
            "shared convention: terse answers",
            "--epistemic-kind",
            "fact",
            "--category",
            "tool_config",
            "--confidence",
            "0.7",
            "--evidence-ref",
            "ev-shared-1",
            "--source-text",
            "The shared convention prefers terse answers (1 note since 2024-01-01).",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["pass"], json!(true), "{payload}");
    assert!(payload["id"].as_str().is_some(), "{payload}");
}

#[test]
fn shared_store_accepts_the_episodic_surface() {
    // `--episodic-type` routes the store to the episodic candidate path
    // (no category, semantic rules do not apply).
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared store",
        &[
            "shared episodic note: deploy freeze on Fridays",
            "--epistemic-kind",
            "fact",
            "--episodic-type",
            "gist",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["pass"], json!(true), "{payload}");
}

#[test]
fn shared_store_requires_explicit_actor_attribution() {
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared store",
        &[
            "shared convention",
            "--epistemic-kind",
            "fact",
            "--category",
            "tool_config",
            "--shared-bank",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("actor"), "{err}");
}

#[test]
fn shared_discard_previews_without_confirm_then_discards() {
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared store",
        &[
            "shared convention: retire me",
            "--epistemic-kind",
            "fact",
            "--category",
            "tool_config",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = json_of(&out)["id"].as_str().expect("stored id").to_owned();
    // Preview: no writes, the item stays retrievable.
    let (code, out, err) = run_write_state(
        "shared discard",
        &[
            &id,
            "--reason",
            "retired",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains(&format!("preview: would discard {id} (pass --confirm)")),
        "{out}"
    );
    assert!(
        s.store.get_memory_item("shared", &id).unwrap().is_some(),
        "preview must not remove the item"
    );
    // Confirmed: the tool removes it from active use.
    let (code, out, err) = run_write_state(
        "shared discard",
        &[
            &id,
            "--reason",
            "retired",
            "--confirm",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains(&format!("discarded {id}")), "{out}");
}

#[test]
fn shared_discard_requires_reason() {
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared discard",
        &["itm-x", "--actor", "agent", "--shared-bank"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--reason"), "{err}");
}

#[test]
fn text_mode_shared_store_renders_the_stored_shared_item() {
    let s = shared_state();
    let (code, out, err) = run_write_state(
        "shared store",
        &[
            "shared convention: aisle seats",
            "--epistemic-kind",
            "fact",
            "--category",
            "tool_config",
            "--actor",
            "agent",
            "--shared-bank",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("stored shared item itm-cli-"), "{out}");
    assert!(out.contains("admission_score"), "{out}");
}

#[test]
fn render_views_fall_back_deterministically() {
    assert_eq!(
        super::render_shared_stored(&json!({"id": "itm-sh"})),
        "stored shared item itm-sh"
    );
    assert_eq!(
        super::render_discard(&json!({"confirm": false, "would_discard": {"item_id": "itm-1"}})),
        "preview: would discard itm-1 (pass --confirm)"
    );
    assert_eq!(
        super::render_discard(&json!({"confirm": true, "discarded": {"item_id": "itm-1"}})),
        "discarded itm-1"
    );
    assert_eq!(
        super::render_discard(&json!({})),
        "preview: would discard ? (pass --confirm)"
    );
    assert_eq!(
        super::render_validated(&json!({"item_id": "itm-1", "action": "update", "applied": true})),
        "validated itm-1 action=update applied=true"
    );
    assert_eq!(
        super::render_validated(&json!({})),
        "validated ? action=? applied=?"
    );
}

#[test]
fn shared_store_help_exits_zero_before_opening_a_store() {
    let (code, out, err) = run_write_resolved("shared store", &["--help"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("usage: clio shared store TEXT"), "{out}");
    assert!(out.contains("`shared_store` tool"), "{out}");
}
