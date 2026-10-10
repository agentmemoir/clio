// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Per-bank retention profile integration harness: monotonicity, batch dry-run,
//! override isolation, invalid/oversize rejection, bank isolation, upgrade
//! defaults, and the `clio retention` CLI surface.

use clio::retention::{RETENTION_PROFILE_VERSION, RetentionProfile, Verbosity};
use clio::{Runtime, ToolResult};
use clio_mcp::retention_tools::MAX_PREVIEW_BATCH;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

fn clio_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clio"))
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Routine status candidate with no snapshot: only the text drives the score.
fn candidate(id: &str, text: &str) -> Value {
    json!({
        "item": {"id": id, "gist": text},
        "episodic_type": "gist",
        "epistemic_kind": "fact"
    })
}

/// 20 candidates: half carry decision markers (high utility), half are chatter.
fn twenty_item_fixture() -> Vec<Value> {
    (0..20)
        .map(|i| {
            if i % 2 == 0 {
                candidate(&format!("f{i}"), &format!("must require decision item {i}"))
            } else {
                candidate(&format!("f{i}"), "ok")
            }
        })
        .collect()
}

fn inspect_count(state: &McpState, bank: &str) -> usize {
    let out = call(state, "inspect", json!({"bank": bank, "limit": 200}));
    out["items"].as_array().map_or(0, Vec::len)
}

fn preview(
    state: &McpState,
    bank: &str,
    items: &[Value],
    override_profile: Option<Value>,
) -> Value {
    let mut args = json!({"bank": bank, "actor": "agent", "items": items});
    if let Some(profile) = override_profile {
        args["profile_override"] = profile;
    }
    call(state, "admit_preview_batch", args)
}

#[test]
fn t25_01_verbosity_admit_sets_are_monotone() {
    let s = state();
    let fixture = twenty_item_fixture();
    let admit_count = |verbosity: &str| {
        let out = preview(
            &s,
            "bank-a",
            &fixture,
            Some(json!({"verbosity": verbosity})),
        );
        assert_eq!(out["ok"], json!(true), "{out}");
        for (i, item) in out["items"].as_array().expect("items").iter().enumerate() {
            if item["pass"] == json!(false) {
                assert!(item["rejection_reason"].is_string(), "item {i}: {item}");
            }
        }
        out["summary"]["would_admit"].as_u64().expect("admit")
    };
    let selective = admit_count("selective");
    let balanced = admit_count("balanced");
    let permissive = admit_count("permissive");
    assert!(
        selective < balanced,
        "selective {selective} vs balanced {balanced}"
    );
    assert!(
        balanced <= permissive,
        "balanced {balanced} vs permissive {permissive}"
    );
    assert_eq!(balanced, 20, "balanced admits the whole fixture");
}

#[test]
fn t25_02_batch_preview_reports_decisions_and_writes_nothing() {
    let s = state();
    let before = inspect_count(&s, "bank-a");
    let fixture = twenty_item_fixture();
    let mixed: Vec<Value> = fixture.into_iter().take(10).collect();
    let out = preview(&s, "bank-a", &mixed, None);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["count"], json!(10));
    assert_eq!(out["dry_run"], json!(true));
    assert_eq!(out["writes"], json!(0));
    assert_eq!(out["items"].as_array().expect("items").len(), 10);
    let admit = out["summary"]["would_admit"].as_u64().expect("admit");
    let reject = out["summary"]["would_reject"].as_u64().expect("reject");
    assert_eq!(admit + reject, 10);
    assert_eq!(inspect_count(&s, "bank-a"), before, "preview wrote an item");
    assert!(
        s.store
            .get_memory_item("bank-a", "f0")
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn t25_03_profile_override_does_not_persist() {
    let s = state();
    let set = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "balanced"}, "confirm": true}),
    );
    assert_eq!(set["ok"], json!(true), "{set}");

    let out = preview(
        &s,
        "bank-a",
        &twenty_item_fixture(),
        Some(json!({"verbosity": "selective"})),
    );
    assert_eq!(out["profile_source"], json!("override"));
    assert!(
        out["summary"]["would_admit"].as_u64().expect("admit") < 20,
        "override must apply to this response"
    );

    let after = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(
        after["verbosity"],
        json!("balanced"),
        "override must not persist"
    );
    let preview_again = preview(&s, "bank-a", &twenty_item_fixture(), None);
    assert_eq!(preview_again["summary"]["would_admit"], json!(20));
}

#[test]
fn t25_04_invalid_profiles_are_rejected_and_stored_profile_is_unchanged() {
    let s = state();
    for bad in [
        json!({"verbosity": "verbose"}),
        json!({"theta_offsets": {"persona": 0.9}}),
        json!({"theta_offsets": {"not_a_key": 0.1}}),
        json!({"duplicate_tolerance_write": "sometimes"}),
        json!({"version": 99}),
    ] {
        let out = call(
            &s,
            "retention_profile_set",
            json!({"bank": "bank-a", "profile": bad, "confirm": true}),
        );
        assert_eq!(out["ok"], json!(false), "accepted invalid profile: {out}");
    }
    let get = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(get["source"], json!("builtin"));
    assert_eq!(get["version"], json!(RETENTION_PROFILE_VERSION));
}

#[test]
fn t25_05_oversize_batch_is_a_clean_limit_error() {
    let s = state();
    let before = inspect_count(&s, "bank-a");
    let items: Vec<Value> = (0..=MAX_PREVIEW_BATCH)
        .map(|i| candidate(&format!("o{i}"), "ok"))
        .collect();
    let out = preview(&s, "bank-a", &items, None);
    assert_eq!(out["ok"], json!(false));
    assert_eq!(out["code"], json!("invalid_argument"));
    assert!(out.get("items").is_none(), "oversize batch must not score");
    assert_eq!(inspect_count(&s, "bank-a"), before);
}

#[test]
fn t25_06_cross_bank_profile_isolation() {
    let s = state();
    let set = call(
        &s,
        "retention_profile_set",
        json!({"bank": "bank-a", "profile": {"verbosity": "selective"}, "confirm": true}),
    );
    assert_eq!(set["ok"], json!(true));
    let a = call(&s, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(a["source"], json!("bank"));
    assert_eq!(a["verbosity"], json!("selective"));
    let b = call(&s, "retention_profile_get", json!({"bank": "bank-b"}));
    assert_eq!(b["source"], json!("builtin"));
    assert_eq!(b["verbosity"], json!("balanced"));
    // A bank-B preview is unaffected by bank A's strictness.
    let out = preview(&s, "bank-b", &twenty_item_fixture(), None);
    assert_eq!(out["summary"]["would_admit"], json!(20));
}

#[test]
fn t25_07_fresh_runtime_resolves_builtin_default_and_still_gates() {
    let s = state();
    let get = call(
        &s,
        "retention_profile_get",
        json!({"bank": "never-configured"}),
    );
    assert_eq!(get["source"], json!("builtin"));
    assert_eq!(get["verbosity"], json!("balanced"));
    assert_eq!(
        get["accepted_but_inert"]["honored_by"],
        json!("recall_scope_and_dedup")
    );

    // A gated write still returns a structured rejection with no stored item.
    let refused = call(
        &s,
        "store",
        json!({
            "item": {
                "id": "itm-refused",
                "gist": "unverifiable snapshot",
                "snapshot": {"entity": "Ada Lovelace", "amount": 42, "date": "1852-01-01"}
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "unrelated text with no matching span",
            "bank": "never-configured",
            "actor": "agent"
        }),
    );
    // A refused write is `ok:false`, so it is never read as stored.
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert_eq!(refused["code"], json!("admission_refused"), "{refused}");
    assert_eq!(refused["pass"], json!(false));
    assert!(refused["rejection_reason"].is_string());
    assert_eq!(refused["profile_version"], json!(RETENTION_PROFILE_VERSION));
    assert!(
        s.store
            .get_memory_item("never-configured", "itm-refused")
            .expect("lookup")
            .is_none()
    );
}

#[test]
fn t25_08_admission_decisions_carry_the_resolved_profile_version() {
    let s = state();
    let out = preview(
        &s,
        "bank-a",
        &[candidate("pv1", "must require decision")],
        None,
    );
    assert_eq!(out["profile_version"], json!(RETENTION_PROFILE_VERSION));
    assert_eq!(
        out["items"][0]["profile_version"],
        json!(RETENTION_PROFILE_VERSION)
    );
}

#[test]
fn runtime_profile_store_round_trips_through_a_file() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("am-harness-retention-{nanos}.json"));

    let mut rt = Runtime::with_env(&|_| None);
    rt.set_retention_path(path.clone());
    let ToolResult::Ok { data, .. } = rt.retention_profile_set(
        "bank-a",
        RetentionProfile {
            verbosity: Verbosity::Selective,
            ..RetentionProfile::default()
        },
    ) else {
        panic!("set");
    };
    assert!(data.persisted);

    let mut restarted = Runtime::with_env(&|_| None);
    restarted.set_retention_path(path.clone());
    let ToolResult::Ok { data, .. } = restarted.retention_profile_get("bank-a") else {
        panic!("get");
    };
    assert_eq!(data.source, "bank");
    assert_eq!(data.verbosity, "selective");
    let ToolResult::Ok { data, .. } = restarted.retention_profile_get("bank-b") else {
        panic!("get b");
    };
    assert_eq!(data.source, "builtin");

    let _ = std::fs::remove_file(path);
}

#[test]
fn retention_profile_strictness_applies_across_a_restart() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("am-harness-restart-{nanos}"));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("profiles.json");

    // Persist a selective profile for bank-a to the durable store file.
    let mut writer = Runtime::with_env(&|_| None);
    writer.set_retention_path(file.clone());
    let ToolResult::Ok { data, .. } = writer.retention_profile_set(
        "bank-a",
        RetentionProfile {
            verbosity: Verbosity::Selective,
            ..RetentionProfile::default()
        },
    ) else {
        panic!("set");
    };
    assert!(data.persisted);
    drop(writer);

    // A fresh MCP runtime loads the persisted file: the bank is strict again.
    let mut strict =
        McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
            .expect("state");
    Arc::get_mut(&mut strict)
        .expect("unique state")
        .set_retention_path(file);
    let get = call(&strict, "retention_profile_get", json!({"bank": "bank-a"}));
    assert_eq!(get["source"], json!("bank"), "{get}");
    assert_eq!(get["verbosity"], json!("selective"));
    let strict_out = preview(&strict, "bank-a", &twenty_item_fixture(), None);
    let strict_admit = strict_out["summary"]["would_admit"]
        .as_u64()
        .expect("admit");
    assert!(
        strict_admit < 20,
        "selective must reject part of the fixture after restart: {strict_admit}"
    );

    // A runtime with no stored profile admits the whole fixture.
    let fresh = state();
    let fresh_out = preview(&fresh, "bank-a", &twenty_item_fixture(), None);
    assert_eq!(fresh_out["summary"]["would_admit"], json!(20));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cli_retention_schema_set_and_get_round_trip() {
    let schema = clio_bin()
        .args(["retention", "schema"])
        .output()
        .expect("run schema");
    assert!(schema.status.success());
    let schema_body = String::from_utf8_lossy(&schema.stdout);
    assert!(schema_body.contains("x-verbosity-mapping"));

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("am-cli-harness-retention-{nanos}.json"));
    let path_str = path.to_string_lossy().into_owned();

    let set = clio_bin()
        .args([
            "retention",
            "set",
            "--bank",
            "bank-a",
            "--profiles",
            &path_str,
            "--profile",
            r#"{"verbosity":"selective"}"#,
        ])
        .output()
        .expect("run set");
    assert!(
        set.status.success(),
        "set failed: {}",
        String::from_utf8_lossy(&set.stderr)
    );

    let get = clio_bin()
        .args([
            "retention",
            "get",
            "--bank",
            "bank-a",
            "--profiles",
            &path_str,
        ])
        .output()
        .expect("run get");
    assert!(get.status.success());
    let body = String::from_utf8_lossy(&get.stdout);
    assert!(body.contains("\"verbosity\": \"selective\""), "{body}");
    assert!(body.contains("honored_by"), "{body}");

    let bad = clio_bin()
        .args([
            "retention",
            "set",
            "--bank",
            "bank-a",
            "--profiles",
            &path_str,
            "--profile",
            r#"{"verbosity":"verbose"}"#,
        ])
        .output()
        .expect("run bad set");
    assert_eq!(bad.status.code(), Some(1));

    let _ = std::fs::remove_file(path);
}
