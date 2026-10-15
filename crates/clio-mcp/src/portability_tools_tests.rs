// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the JSON portability
//! tools: bundle export/import dispatch, authorization denial, idempotent
//! re-import, and the `import_provider` first-release stub.
//!
//! ## Owns
//! - Dispatcher-level assertions for `export` / `import` / `import_provider`
//!
//! ## Does not own
//! - In-process export/import semantics (see `clio-compliance` export/import tests)
//! - Shared state/dispatch fixtures (see `additive_tools_test_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_store::LocalEventIntent;
use serde_json::{Value, json};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this process, so this helper never hands two
/// callers the same path.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Default sqlite-backed test state (matches the additive fixture builder).
pub(crate) fn state() -> std::sync::Arc<crate::runtime::McpState> {
    crate::runtime::McpState::open_with_effective(
        &crate::runtime::McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &crate::runtime::McpState, name: &str, args: Value) -> Value {
    crate::write_tools::dispatch(state, name, &args)
}

fn temp_path(label: &str) -> String {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    path.push(format!(
        "am-port-{label}-{}-{nanos}-{}.json",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    path.to_string_lossy().into_owned()
}

fn store_probe(state: &crate::runtime::McpState, id: &str) -> Value {
    call(
        state,
        "store",
        serde_json::json!({
            "item": {
                "id": id,
                "subject_id": format!("subj-{id}"),
                "gist": "portability probe item zzqxj",
                "snapshot": {"entity": "probe", "amount": 3}
            },
            "category": "schema",
            "epistemic_kind": "fact",
            "source_text": "portability probe zzqxj (3 notes since 2026-01-01).",
            "bank": "bank-a",
            "actor": "agent"
        }),
    )
}

// ---------- export ----------

#[test]
fn export_writes_bundle_manifest_for_privileged_actor() {
    let s = state();
    assert_eq!(store_probe(&s, "pexp-1")["ok"], Value::Bool(true));
    let dest = temp_path("ok");
    let out = call(
        &s,
        "export",
        json!({
            "destination": dest,
            "bank": "bank-a",
            "actor": "harness"
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["content_mode"], "dsar_plaintext");
    let manifest = &out["manifest"];
    assert_eq!(manifest["complete"], Value::Bool(true));
    let text = fs::read_to_string(&dest).expect("bundle file");
    assert!(text.contains("clio.memory_export"));
    fs::remove_file(&dest).ok();
}

#[test]
fn export_denies_unprivileged_actor_without_file() {
    let s = state();
    let dest = temp_path("denied");
    let out = call(
        &s,
        "export",
        json!({"destination": dest, "bank": "bank-a", "actor": "agent"}),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert!(!std::path::Path::new(&dest).exists());
}

#[test]
fn export_rejects_an_unknown_category_without_writing_a_bundle() {
    let s = state();
    let dest = temp_path("bad-category");
    let out = call(
        &s,
        "export",
        json!({
            "destination": dest,
            "category": "bogus",
            "bank": "bank-a",
            "actor": "harness"
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "invalid_argument", "{out}");
    assert!(!std::path::Path::new(&dest).exists());
}

#[test]
fn export_rejects_an_invalid_time_bound() {
    assert!(crate::write_tools::validate_time_bound("time_start", "2026-01-01").is_ok());
    assert!(crate::write_tools::validate_time_bound("time_end", "2026-01-01T00:00:00Z").is_ok());
    let s = state();
    let dest = temp_path("bad-time");
    let out = call(
        &s,
        "export",
        json!({
            "destination": dest,
            "time_start": "not-a-time",
            "bank": "bank-a",
            "actor": "harness"
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "invalid_argument", "{out}");
    assert!(!std::path::Path::new(&dest).exists());
}

// ---------- import ----------

#[test]
fn import_dry_run_then_live_then_reimport_idempotent() {
    let s = state();
    assert_eq!(store_probe(&s, "pimp-1")["ok"], Value::Bool(true));
    let dest = temp_path("idem");
    let exported = call(
        &s,
        "export",
        json!({"destination": dest, "bank": "bank-a", "actor": "harness"}),
    );
    assert_eq!(exported["ok"], Value::Bool(true), "{exported}");

    let dry = call(
        &s,
        "import",
        json!({"source": dest, "dry_run": true, "bank": "bank-a", "actor": "harness"}),
    );
    assert_eq!(dry["ok"], Value::Bool(true), "{dry}");
    let report = &dry["report"];
    assert_eq!(report["dry_run"], Value::Bool(true));
    assert_eq!(report["would_skip"], Value::Number(1.into()));

    let live = call(
        &s,
        "import",
        json!({"source": dest, "bank": "bank-a", "actor": "harness"}),
    );
    assert_eq!(live["ok"], Value::Bool(true), "{live}");
    assert_eq!(live["report"]["applied"], Value::Bool(true));

    let again = call(
        &s,
        "import",
        json!({"source": dest, "bank": "bank-a", "actor": "harness"}),
    );
    let again_report = &again["report"];
    assert_eq!(again_report["would_skip"], Value::Number(1.into()));
    assert_eq!(again_report["would_create"], Value::Number(0.into()));
    fs::remove_file(&dest).ok();
}

#[test]
fn import_force_without_reason_is_rejected() {
    let s = state();
    let dest = temp_path("force");
    let out = call(
        &s,
        "import",
        json!({"source": dest, "force": true, "bank": "bank-a", "actor": "harness"}),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
}

#[test]
fn import_rejects_unsupported_format() {
    let s = state();
    let src = temp_path("badformat");
    fs::write(&src, r#"{"format":"alien","format_version":1}"#).expect("write");
    let out = call(
        &s,
        "import",
        json!({"source": src, "bank": "bank-a", "actor": "harness"}),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    fs::remove_file(&src).ok();
}

// ---------- import_provider stub ----------

#[test]
fn t22_07_import_provider_stub_unsupported_with_masked_credentials() {
    let s = state();
    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "openai",
            "credentials": {"api_key": "sk-test-1234567890"},
            "bank": "bank-a",
            "actor": "harness"
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "PROVIDER_IMPORT_UNSUPPORTED");
    assert_eq!(out["writes_performed"], 0);
    let masked = out["credentials_masked"]["****_key"]
        .as_str()
        .expect("masked credential echoed");
    assert!(masked.starts_with("****"));
    assert!(!out.to_string().contains("sk-test-1234567890"));
}

#[test]
fn bound_catalog_contains_portability_tools() {
    assert!(crate::bound_write_tools().contains(&"export"));
    assert!(crate::bound_write_tools().contains(&"import"));
    assert!(crate::bound_write_tools().contains(&"import_provider"));
}

#[test]
fn portability_dispatch_unknown_tool_and_missing_provider() {
    let s = state();
    let err = crate::portability_tools::dispatch_inner(&s, "unknown_tool", &json!({})).unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::NotImplemented);

    let err2 = call(&s, "import_provider", json!({"actor": "harness"}));
    assert_eq!(err2["ok"], Value::Bool(false));
}

// ---------- review_export ----------

/// Seed one completed local event row. The outcome `result` carries a marker so
/// the review export can be checked for content-freeness.
fn seed_review_event(state: &crate::runtime::McpState, action_id: &str, denied: bool) {
    state
        .events
        .record_intent(&LocalEventIntent {
            action_id,
            actor: "harness",
            scope: "review",
            kind: "export",
            event_time: "2026-01-01T00:00:00Z",
        })
        .expect("record intent");
    state
        .events
        .record_outcome(action_id, denied, "SECRET-MARKER")
        .expect("record outcome");
}

#[test]
fn review_export_is_listed_in_the_portability_schema() {
    let defs = crate::schema_portability_defs::portability_defs();
    let def = defs
        .iter()
        .find(|d| d.name == "review_export")
        .expect("review_export schema");
    assert_eq!(def.required, &["destination"][..]);
    let names: Vec<&str> = def.props.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, vec!["destination", "limit", "cursor"]);
}

#[test]
fn review_export_writes_ordered_content_free_rows() {
    let s = state();
    seed_review_event(&s, "rev-allowed", false);
    seed_review_event(&s, "rev-denied", true);
    let dest = temp_path("review-rows");
    let out = call(
        &s,
        "review_export",
        json!({
            "destination": dest,
            "bank": "bank-a",
            "actor": "harness",
            "timestamp": "2026-01-01T00:00:00Z"
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["destination"], Value::from(dest.as_str()));
    let text = fs::read_to_string(&dest).expect("review export file");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].ends_with("rev-allowed\tallowed"), "{text}");
    assert!(lines[1].ends_with("rev-denied\tdenied"), "{text}");
    assert!(
        lines.iter().all(|line| line.split('\t').count() == 3),
        "{text}"
    );
    assert!(!text.contains("SECRET-MARKER"), "content-free: {text}");
    fs::remove_file(&dest).ok();
}

#[test]
fn review_export_refuses_an_existing_destination_without_overwriting() {
    let s = state();
    seed_review_event(&s, "rev-exists", false);
    let dest = temp_path("review-exists");
    fs::write(&dest, "KEEP").expect("seed destination");
    let out = call(
        &s,
        "review_export",
        json!({"destination": dest, "bank": "bank-a", "actor": "harness"}),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "invalid_argument", "{out}");
    assert_eq!(fs::read_to_string(&dest).expect("kept file"), "KEEP");
    fs::remove_file(&dest).ok();
}

#[test]
fn review_export_missing_destination_is_a_usage_error() {
    let s = state();
    let out = call(
        &s,
        "review_export",
        json!({"bank": "bank-a", "actor": "harness"}),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "invalid_argument", "{out}");
}
