// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the human text views of the `clio sync`
//! verbs: status, push/pull, and serve.
//!
//! ## Owns
//! - `render`, the dispatch from a verb to its text view
//! - The three text views and the cursor helper they share
//!
//! ## Does not own
//! - The verbs, their flags, and their payloads (see `sync_cli`)
//! - JSON output (see `cli_output`)
//!
//! ## Boundary
//!
//! Changes to what a sync verb prints in text mode should normally require
//! changes here. Changes to what a verb does should not.
//!
//! Keep this module focused on the text views.
//!
//! This split exists because `sync_cli` was within two lines of the workspace
//! file-size limit when the key verbs landed beside it, and the views are the
//! cohesive half that does not touch the store.

use serde_json::Value;

/// Human text for a sync success payload.
pub(crate) fn render(verb: &str, payload: &Value) -> String {
    match verb {
        "sync status" => render_status(payload),
        "sync serve" => render_serve(payload),
        _ => render_transfer(payload),
    }
}

/// Human text for `sync status`: endpoint/auth state, then one line per bank.
fn render_status(payload: &Value) -> String {
    let device = payload
        .get("device_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let remote = payload
        .get("remote")
        .and_then(Value::as_str)
        .unwrap_or("(none)");
    let auth = payload
        .get("auth")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let encryption = payload
        .get("encryption")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut lines = vec![format!(
        "sync status (device {device}, remote {remote}, auth {auth}, encryption {encryption})"
    )];
    if let Some(banks) = payload.get("banks").and_then(Value::as_array) {
        for bank in banks {
            let bank_id = bank.get("bank_id").and_then(Value::as_str).unwrap_or("?");
            let push = cursor(bank, "push_cursor");
            let pull = cursor(bank, "pull_cursor");
            let pending = bank
                .get("pending_push")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let conflicts = bank.get("conflicts").and_then(Value::as_u64).unwrap_or(0);
            let dead = bank
                .get("dead_letter_open")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            lines.push(format!(
                "  {bank_id}: push={push} pull={pull} pending={pending} conflicts={conflicts} dead_letters={dead}"
            ));
        }
    }
    lines.join("\n")
}

/// A cursor field rendered as its string, or `-` when unset.
fn cursor(bank: &Value, key: &str) -> String {
    bank.get(key)
        .and_then(Value::as_str)
        .map_or_else(|| "-".to_owned(), str::to_owned)
}

/// Human text for `sync push` / `sync pull`.
fn render_transfer(payload: &Value) -> String {
    let mode = payload.get("mode").and_then(Value::as_str).unwrap_or("?");
    let remote = payload
        .get("remote")
        .and_then(Value::as_str)
        .unwrap_or("(none)");
    let token = payload.get("token").and_then(Value::as_str).unwrap_or("-");
    let encryption = payload
        .get("encryption")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut lines = vec![format!(
        "sync {mode} (remote {remote}, token {token}, encryption {encryption})"
    )];
    if let Some(banks) = payload.get("banks").and_then(Value::as_array) {
        for bank in banks {
            let bank_id = bank.get("bank_id").and_then(Value::as_str).unwrap_or("?");
            let push = cursor(bank, "push_cursor");
            let pull = cursor(bank, "pull_cursor");
            lines.push(format!("  {bank_id}: push={push} pull={pull}"));
        }
    }
    lines.join("\n")
}

/// Human text for `sync serve`.
fn render_serve(payload: &Value) -> String {
    let addr = payload.get("addr").and_then(Value::as_str).unwrap_or("?");
    let ttl = payload.get("ttl_secs").and_then(Value::as_u64).unwrap_or(0);
    let dev = payload.get("dev").and_then(Value::as_bool).unwrap_or(false);
    let banks = payload
        .get("banks")
        .and_then(Value::as_array)
        .map_or_else(String::new, |banks| {
            banks
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",")
        });
    format!(
        "sync serve listening on {addr} (banks {banks}, ttl {ttl}s, dev {dev}); \
         press Ctrl-C to stop early"
    )
}

#[cfg(test)]
#[path = "sync_cli_render_tests.rs"]
mod tests;
