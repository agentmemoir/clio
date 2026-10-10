// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the MCP `summarize` surface:
//! gist regeneration, snapshot immutability, the no-write `dry_run` path,
//! fail-closed behavior without a gist generator, scope resolution, and bank
//! isolation.
//!
//! ## Owns
//! - Test doubles for the extraction seam (stub extractor, stub transport)
//! - Assertions over the structured summarize report
//!
//! ## Does not own
//! - Production summarize behavior (see `summarize_tools`)
//! - The extraction adapters under test (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to summarize's observable report shape should normally require
//! changes here. Changes to extraction adapter internals should not.
//!
//! Keep this module focused on summarize surface tests.

use super::*;
use crate::runtime::{McpOpenOptions, McpState};
use clio_types::{AmError, ToolResult};
use clio_write::{ExtractCandidate, ExtractRequest, Transport};
use serde_json::{Value, json};

fn state() -> std::sync::Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// Store one admitted item with a snapshot through the real MCP write path.
fn store_item(state: &McpState, bank: &str, id: &str, with_snapshot: bool) {
    // The gist carries the id: several items in one bank sharing one gist are
    // exact duplicates, and the unconditional write-side rule refuses the
    // second store.
    let mut item =
        json!({"id": id, "subject_id": format!("subj-{id}"), "gist": format!("old prose {id}")});
    if with_snapshot {
        item["snapshot"] = json!({"entity": "dark mode", "amount": 1, "date": "2024-01-01"});
    }
    let mut args = json!({
        "item": item,
        "category": "task_spec",
        "epistemic_kind": "fact",
        "bank": bank,
        "actor": "agent",
        "timestamp": "2024-01-01T00:00:00Z"
    });
    if with_snapshot {
        args["source_text"] = json!("The user prefers dark mode (1 note since 2024-01-01).");
    }
    let out = crate::write_tools::dispatch(state, "store", &args);
    assert_eq!(out["ok"], json!(true), "store {id}: {out}");
}

/// Stub gist generator: echoes the source-text length so the test can prove
/// the snapshot was the summarization input.
#[derive(Debug)]
struct RegenExtractor;

impl Extractor for RegenExtractor {
    fn extract(&self, req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
        Ok(ExtractCandidate {
            snapshot: None,
            gist: Some(format!("regen[{}]", req.source_text.len())),
            source_ref: "summarize-test".to_owned(),
            diagnostic: None,
        })
    }
}

/// Stub HTTP transport for the configured-provider path: returns a gist without
/// any network I/O, so `build_extractor_from` + `TemplateApiExtractor` are the
/// code under test rather than a hand-rolled extractor.
#[derive(Debug)]
struct StubTransport;

impl Transport for StubTransport {
    fn post_json(&self, _url: &str, _body: &Value) -> Result<Value, AmError> {
        Ok(json!({"gist": "stub-sidecar gist", "source_ref": "stub"}))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn call_summarize(state: &McpState, args: Value, extractor: &dyn Extractor) -> Value {
    // Shape the direct-call result exactly as the MCP dispatcher would.
    match crate::summarize_tools::summarize_with_extractor(state, &args, extractor) {
        Ok(data) => serde_json::to_value(ToolResult::success(data)).expect("serialize report"),
        Err(e) => serde_json::to_value(ToolResult::<()>::failure(e)).expect("serialize error"),
    }
}

/// Run through the real configured-extractor path with a stub transport.
#[allow(clippy::needless_pass_by_value)]
fn call_summarize_configured(state: &McpState, args: Value) -> Value {
    match crate::summarize_tools::summarize_with_transport(state, &args, StubTransport) {
        Ok(data) => serde_json::to_value(ToolResult::success(data)).expect("serialize report"),
        Err(e) => serde_json::to_value(ToolResult::<()>::failure(e)).expect("serialize error"),
    }
}

fn snapshot_bytes(state: &McpState, bank: &str, id: &str) -> String {
    let snapshot = state
        .store
        .get_snapshot(bank, id)
        .expect("snapshot read")
        .expect("snapshot present");
    serde_json::to_string(&snapshot).expect("serialize snapshot")
}

fn stored_gist(state: &McpState, bank: &str, id: &str) -> String {
    crate::write_tools::dispatch(state, "get_gist", &json!({"bank": bank, "item_id": id}))["gist"]
        .as_str()
        .unwrap_or("?")
        .to_owned()
}

/// `summarize` over MCP on an item regenerates its gist into the structured
/// report.
#[test]
fn summarize_item_regenerates_gist() {
    let s = state();
    store_item(&s, "bank-a", "itm-1", true);
    let report = call_summarize(
        &s,
        json!({"scope": "itm-1", "bank": "bank-a"}),
        &RegenExtractor,
    );
    assert_eq!(report["ok"], json!(true), "{report}");
    assert_eq!(report["scope"], json!("itm-1"));
    assert_eq!(report["count"]["summarized"], json!(1));
    assert_eq!(report["count"]["skipped"], json!(0));
    assert_eq!(report["dry_run"], json!(false));
    assert!(
        report["summarized"][0]["gist"]
            .as_str()
            .unwrap_or_default()
            .starts_with("regen["),
        "{report}"
    );
    assert_ne!(
        stored_gist(&s, "bank-a", "itm-1"),
        "old prose itm-1",
        "the stored gist must change"
    );
}

/// Snapshots are byte-identical before and after `summarize`.
#[test]
fn snapshots_are_byte_identical() {
    let s = state();
    store_item(&s, "bank-a", "itm-2", true);
    let before = snapshot_bytes(&s, "bank-a", "itm-2");
    let report = call_summarize(
        &s,
        json!({"scope": "itm-2", "bank": "bank-a"}),
        &RegenExtractor,
    );
    assert_eq!(report["ok"], json!(true), "{report}");
    let after = snapshot_bytes(&s, "bank-a", "itm-2");
    assert_eq!(before, after, "summarize must never alter snapshots");
}

/// With no gist generator configured the tool fails closed with a structured
/// error and fabricates no text.
#[test]
fn fail_closed_without_generator() {
    let s = state();
    let out = crate::write_tools::dispatch(&s, "summarize", &json!({"scope": "bank"}));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_implemented"), "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or_default()
            .contains("extraction unavailable"),
        "{out}"
    );
}

/// The configured provider path (factory + `TemplateApiExtractor`) runs end to
/// end with a stub transport and persists the extracted gist.
#[test]
fn configured_extractor_path_persists_gist() {
    let s = state();
    let set = crate::write_tools::dispatch(
        &s,
        "config_set",
        &json!({"path": "extract.url", "value": "http://127.0.0.1:9/extract"}),
    );
    assert_eq!(set["ok"], json!(true), "configure extract.url: {set}");
    store_item(&s, "bank-a", "itm-cfg", true);
    let report = call_summarize_configured(&s, json!({"scope": "itm-cfg", "bank": "bank-a"}));
    assert_eq!(report["ok"], json!(true), "{report}");
    assert_eq!(report["count"]["summarized"], json!(1), "{report}");
    assert_eq!(report["summarized"][0]["gist"], json!("stub-sidecar gist"));
    assert_eq!(
        stored_gist(&s, "bank-a", "itm-cfg"),
        "stub-sidecar gist",
        "the configured path must persist the extracted gist"
    );
}

/// `dry_run=true` reports the gist it would regenerate but never persists it.
#[test]
fn dry_run_reports_without_writing() {
    let s = state();
    store_item(&s, "bank-a", "itm-dry", true);
    let report = call_summarize(
        &s,
        json!({"scope": "itm-dry", "bank": "bank-a", "dry_run": true}),
        &RegenExtractor,
    );
    assert_eq!(report["ok"], json!(true), "{report}");
    assert_eq!(report["dry_run"], json!(true));
    assert_eq!(report["count"]["summarized"], json!(1), "{report}");
    assert!(
        report["summarized"][0]["gist"]
            .as_str()
            .unwrap_or_default()
            .starts_with("regen["),
        "{report}"
    );
    assert_eq!(
        stored_gist(&s, "bank-a", "itm-dry"),
        "old prose itm-dry",
        "dry_run must not write"
    );
}

/// Bank scope summarizes every snapshot-bearing active item and reports
/// archived/discarded items as skipped with reasons.
#[test]
fn bank_scope_reports_summarized_and_skipped() {
    let s = state();
    store_item(&s, "bank-a", "itm-3", true);
    store_item(&s, "bank-a", "itm-4", false);
    store_item(&s, "bank-a", "itm-arch", true);
    store_item(&s, "bank-a", "itm-disc", true);
    s.hygiene
        .archive_item("bank-a", "itm-arch", "2024-02-01T00:00:00Z", "agent")
        .expect("archive");
    s.store
        .discard_item(
            "bank-a",
            "itm-disc",
            "noise",
            "2024-02-01T00:00:00Z",
            "agent",
        )
        .expect("discard");
    let report = call_summarize(
        &s,
        json!({"scope": "bank", "bank": "bank-a"}),
        &RegenExtractor,
    );
    assert_eq!(report["ok"], json!(true), "{report}");
    assert_eq!(report["scope"], json!("bank"));
    assert_eq!(report["count"]["summarized"], json!(1), "{report}");
    assert_eq!(report["count"]["skipped"], json!(3), "{report}");
    let skipped = report["skipped"].as_array().expect("skipped");
    let reason_of = |id: &str| {
        skipped
            .iter()
            .find(|e| e["item_id"] == json!(id))
            .and_then(|e| e["reason"].as_str())
            .unwrap_or("?")
            .to_owned()
    };
    assert_eq!(reason_of("itm-4"), "no snapshot to summarize from");
    assert_eq!(reason_of("itm-arch"), "archived");
    assert_eq!(reason_of("itm-disc"), "discarded");
}

/// An unknown item id and a foreign-bank item are both rejected (bank
/// scoping and authorization of the read path).
#[test]
fn item_scope_is_bank_scoped() {
    let s = state();
    store_item(&s, "bank-a", "itm-5", true);
    let missing = call_summarize(
        &s,
        json!({"scope": "ghost", "bank": "bank-a"}),
        &RegenExtractor,
    );
    assert_eq!(missing["ok"], json!(false), "{missing}");
    assert_eq!(missing["code"], json!("item_not_found"), "{missing}");

    let foreign = call_summarize(
        &s,
        json!({"scope": "itm-5", "bank": "bank-b"}),
        &RegenExtractor,
    );
    assert_eq!(foreign["ok"], json!(false), "{foreign}");
    assert_eq!(foreign["code"], json!("item_not_found"), "{foreign}");
}

/// Item scope skips archived and discarded items (consistent with bank scope)
/// rather than summarizing inactive items.
#[test]
fn item_scope_skips_archived_and_discarded() {
    let s = state();
    store_item(&s, "bank-a", "itm-arch2", true);
    store_item(&s, "bank-a", "itm-disc2", true);
    s.hygiene
        .archive_item("bank-a", "itm-arch2", "2024-02-01T00:00:00Z", "agent")
        .expect("archive");
    s.store
        .discard_item(
            "bank-a",
            "itm-disc2",
            "noise",
            "2024-02-01T00:00:00Z",
            "agent",
        )
        .expect("discard");
    for (id, reason) in [("itm-arch2", "archived"), ("itm-disc2", "discarded")] {
        let report = call_summarize(&s, json!({"scope": id, "bank": "bank-a"}), &RegenExtractor);
        assert_eq!(report["ok"], json!(true), "{id}: {report}");
        assert_eq!(report["count"]["summarized"], json!(0), "{id}: {report}");
        assert_eq!(report["skipped"][0]["item_id"], json!(id), "{id}: {report}");
        assert_eq!(
            report["skipped"][0]["reason"],
            json!(reason),
            "{id}: {report}"
        );
        assert_eq!(
            stored_gist(&s, "bank-a", id),
            format!("old prose {id}"),
            "{id} must not be summarized"
        );
    }
}

/// An extractor failure skips the item with a reason instead of fabricating
/// or failing the whole report.
#[test]
fn extractor_failure_is_reported_per_item() {
    #[derive(Debug)]
    struct FailingExtractor;
    impl Extractor for FailingExtractor {
        fn extract(&self, _req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
            Err(AmError::new(ErrorCode::Internal, "extractor down"))
        }
    }
    let s = state();
    store_item(&s, "bank-a", "itm-6", true);
    let report = call_summarize(
        &s,
        json!({"scope": "itm-6", "bank": "bank-a"}),
        &FailingExtractor,
    );
    assert_eq!(report["ok"], json!(true), "{report}");
    assert_eq!(report["count"]["skipped"], json!(1), "{report}");
    assert!(
        report["skipped"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("extraction failed"),
        "{report}"
    );
}

/// An empty-gist candidate is treated as no gist (never stored as blank).
#[test]
fn blank_gist_candidate_is_skipped() {
    #[derive(Debug)]
    struct BlankGistExtractor;
    impl Extractor for BlankGistExtractor {
        fn extract(&self, _req: &ExtractRequest) -> Result<ExtractCandidate, AmError> {
            Ok(ExtractCandidate {
                snapshot: None,
                gist: Some("   ".to_owned()),
                source_ref: "summarize-test".to_owned(),
                diagnostic: None,
            })
        }
    }
    let s = state();
    store_item(&s, "bank-a", "itm-7", true);
    let report = call_summarize(
        &s,
        json!({"scope": "itm-7", "bank": "bank-a"}),
        &BlankGistExtractor,
    );
    assert_eq!(report["ok"], json!(true), "{report}");
    assert_eq!(
        report["skipped"][0]["reason"],
        json!("extractor produced no gist")
    );
    assert_eq!(
        stored_gist(&s, "bank-a", "itm-7"),
        "old prose itm-7",
        "blank gist must not overwrite"
    );
}
