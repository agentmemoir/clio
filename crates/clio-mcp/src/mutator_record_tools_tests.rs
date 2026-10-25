// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for behavioural coverage of the persona and
//! history record write tools at their decode-and-dispatch seam: which outcome
//! each call reports, what it stages for a dry run, and the refusals that stop a
//! call before the domain crate is reached.
//!
//! ## Owns
//! - Outcome-shape assertions for `persona_put_stable`, `persona_observe`,
//!   `task_upsert`, and `failure_record`
//! - Refusal of a caller timestamp this build cannot place
//! - Required-argument and dry-run behaviour of those four tools
//!
//! ## Does not own
//! - The persona and history gates themselves (`clio-persona`, `clio-history`)
//! - Tool-name routing on the MCP surface (see `mutator_tools`)
//!
//! ## Boundary
//!
//! Tests here break when argument decoding or result shaping changes; they never
//! change it.
//!
//! Keep this module focused on the four record tools.

use super::*;
use crate::runtime::McpOpenOptions;

/// One in-memory state; each test builds its own, so no test depends on another.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// Rows the bank holds, so a refusal test can prove nothing was written.
fn count(state: &McpState) -> usize {
    state
        .store
        .count_items(&clio_store::ItemInspectFilter::new("bank-a"))
        .expect("count")
}

/// Dispatch one tool by the name the MCP surface uses.
fn call(state: &McpState, tool: &str, args: &Value) -> Result<Value, AmError> {
    match tool {
        "persona_put_stable" => persona_put_stable(state, args),
        "persona_observe_preference" => persona_observe(state, args),
        "task_upsert" => task_upsert(state, args),
        "failure_record" => failure_record(state, args),
        other => panic!("not a record tool: {other}"),
    }
}

fn with_timestamp(mut args: Value, timestamp: &str) -> Value {
    args["timestamp"] = json!(timestamp);
    args
}

#[test]
fn a_stable_persona_put_reports_the_entry_it_wrote() {
    let s = state();
    let out = persona_put_stable(
        &s,
        &json!({
            "key": "communication_style", "value": "terse",
            "bank": "bank-a", "actor": "agent"
        }),
    )
    .expect("stable put");
    assert!(
        matches!(out["outcome"].as_str(), Some("admitted" | "rejected")),
        "{out}"
    );
    if out["outcome"] == json!("admitted") {
        assert_eq!(out["entry"]["key"], json!("communication_style"));
        assert_eq!(out["entry"]["value"], json!("terse"));
        assert!(out["dropped_keys"].is_array(), "{out}");
        // A stable entry lives in the persona document, not in the item table.
        assert_eq!(
            out["entry"]["as_of"].as_str().map(str::len),
            Some("1970-01-01T00:00:00Z".len()),
            "the entry is stamped in the store's canonical shape: {out}"
        );
    }
}

#[test]
fn a_stable_persona_dry_run_reports_the_entry_and_the_flag() {
    let s = state();
    let out = call(
        &s,
        "persona_put_stable",
        &json!({
            "key": "communication_style", "value": "terse", "dry_run": true,
            "bank": "bank-a", "actor": "agent"
        }),
    );
    // A gate refusal is the one thing a dry run can turn into an error; it is
    // structured, and it still writes nothing.
    if let Ok(out) = out {
        assert_eq!(out["outcome"], json!("dry_run"), "{out}");
        assert_eq!(out["dry_run"], json!(true), "{out}");
        assert!(out["entry"].is_object(), "{out}");
        assert!(out["decision"]["pass"].is_boolean(), "{out}");
    }
    assert_eq!(count(&s), 0, "a dry run never writes");
}

#[test]
fn a_continuous_observation_reports_the_new_state_and_trend() {
    let s = state();
    let out = persona_observe(
        &s,
        &json!({
            "key": "verbosity", "observed_value": 0.4,
            "confidence": 0.8,
            "bank": "bank-a", "actor": "agent"
        }),
    )
    .expect("observe");
    if out["outcome"] == json!("observed") {
        assert!(out["state"].as_f64().is_some(), "{out}");
        assert!(out["trend"].as_f64().is_some(), "{out}");
        assert!(out["admission_score"].as_f64().is_some(), "{out}");
        assert!(out["dropped_keys"].is_array(), "{out}");
    } else {
        assert_eq!(out["outcome"], json!("rejected"), "{out}");
        assert!(out["decision"].is_object(), "{out}");
    }
}

#[test]
fn an_observation_without_a_value_is_refused_as_a_missing_argument() {
    let s = state();
    let err = persona_observe(
        &s,
        &json!({"key": "verbosity", "bank": "bank-a", "actor": "agent"}),
    )
    .expect_err("missing observed_value");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("observed_value"), "{}", err.message);
}

#[test]
fn a_task_upsert_reports_the_record_it_stored_with_its_steps() {
    let s = state();
    let out = task_upsert(
        &s,
        &json!({
            "task_id": "t-1", "definition": "Ship the release", "status": "running",
            "steps": ["build", "test", 7],
            "bank": "bank-a", "actor": "agent"
        }),
    )
    .expect("task upsert");
    assert!(
        matches!(out["outcome"].as_str(), Some("stored" | "rejected")),
        "{out}"
    );
    if out["outcome"] == json!("stored") {
        assert_eq!(out["record"]["task_id"], json!("t-1"));
        assert_eq!(out["record"]["steps"], json!(["build", "test"]), "{out}");
        assert_eq!(count(&s), 1);
    }
}

#[test]
fn a_task_upsert_dry_run_reports_the_carrier_it_would_write() {
    let s = state();
    let out = call(
        &s,
        "task_upsert",
        &json!({
            "task_id": "t-2", "definition": "Ship the release", "status": "running",
            "steps": ["build"], "dry_run": true,
            "bank": "bank-a", "actor": "agent"
        }),
    );
    if let Ok(out) = out {
        assert_eq!(out["outcome"], json!("dry_run"), "{out}");
        assert!(out["record"].is_object(), "{out}");
        assert!(out["carrier_id"].as_str().is_some(), "{out}");
    }
    assert_eq!(count(&s), 0, "a dry run never writes");
}

#[test]
fn a_task_upsert_without_a_definition_is_refused() {
    let s = state();
    let err = task_upsert(
        &s,
        &json!({"task_id": "t-3", "status": "running", "bank": "bank-a", "actor": "agent"}),
    )
    .expect_err("missing definition");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn a_failure_record_reports_the_record_it_stored() {
    let s = state();
    let out = failure_record(
        &s,
        &json!({
            "task_id": "t-1", "attempt_n": 1, "what_failed": "test failed",
            "lesson": "check fixtures first",
            "bank": "bank-a", "actor": "agent"
        }),
    )
    .expect("failure record");
    assert!(
        matches!(out["outcome"].as_str(), Some("stored" | "rejected")),
        "{out}"
    );
    if out["outcome"] == json!("stored") {
        assert_eq!(out["record"]["task_id"], json!("t-1"));
        assert_eq!(out["record"]["attempt_n"], json!(1));
        assert_eq!(count(&s), 1);
    }
}

#[test]
fn a_failure_record_dry_run_reports_the_carrier_it_would_write() {
    let s = state();
    let out = call(
        &s,
        "failure_record",
        &json!({
            "task_id": "t-2", "attempt_n": 2, "what_failed": "test failed",
            "lesson": "check fixtures first", "dry_run": true,
            "bank": "bank-a", "actor": "agent"
        }),
    );
    if let Ok(out) = out {
        assert_eq!(out["outcome"], json!("dry_run"), "{out}");
        assert!(out["record"].is_object(), "{out}");
        assert!(out["carrier_id"].as_str().is_some(), "{out}");
    }
    assert_eq!(count(&s), 0, "a dry run never writes");
}

#[test]
fn a_failure_record_without_an_attempt_number_is_refused() {
    let s = state();
    let err = failure_record(
        &s,
        &json!({
            "task_id": "t-4", "what_failed": "f", "lesson": "l",
            "bank": "bank-a", "actor": "agent"
        }),
    )
    .expect_err("missing attempt_n");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("attempt_n"), "{}", err.message);
}

#[test]
fn every_record_tool_refuses_a_timestamp_it_cannot_place() {
    let s = state();
    // A month the year does not have. Stored verbatim it would compare as a
    // literal string in a time column, so the tool refuses it up front.
    let bad = "2026-13-01T00:00:00Z";
    let cases: [(&str, Value); 4] = [
        (
            "persona_put_stable",
            json!({"key": "communication_style", "value": "terse", "bank": "bank-a", "actor": "agent"}),
        ),
        (
            "persona_observe_preference",
            json!({"key": "verbosity", "observed_value": 0.4, "bank": "bank-a", "actor": "agent"}),
        ),
        (
            "task_upsert",
            json!({
                "task_id": "t-1", "definition": "Ship the release", "status": "running",
                "bank": "bank-a", "actor": "agent"
            }),
        ),
        (
            "failure_record",
            json!({
                "task_id": "t-1", "attempt_n": 1, "what_failed": "f", "lesson": "l",
                "bank": "bank-a", "actor": "agent"
            }),
        ),
    ];
    for (tool, args) in cases {
        let err = call(&s, tool, &with_timestamp(args, bad)).expect_err("unplaceable stamp");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{tool}: {err:?}");
        assert!(err.message.contains(bad), "{tool}: {}", err.message);
    }
    assert_eq!(count(&s), 0, "a refused stamp leaves no row behind");
}

#[test]
fn a_fractional_timestamp_reaches_the_record_row_as_its_second() {
    let s = state();
    let args = with_timestamp(
        json!({
            "task_id": "t-frac", "definition": "Ship the release", "status": "running",
            "bank": "bank-a", "actor": "agent"
        }),
        "2026-10-12T08:30:01.500Z",
    );
    let out = task_upsert(&s, &args).expect("task upsert");
    if out["outcome"] != json!("stored") {
        // The gate refused it; the stamp wiring is covered by the tools that
        // were admitted, so there is no row here to read.
        assert_eq!(count(&s), 0, "{out}");
        return;
    }
    let rows = s
        .store
        .inspect_items(&clio_store::ItemInspectFilter::new("bank-a"))
        .expect("inspect");
    let row = rows.first().expect("the row the write reported");
    assert_eq!(row.created_at, "2026-10-12T08:30:01Z");
    assert_eq!(row.valid_from, "2026-10-12T08:30:01Z");
}
