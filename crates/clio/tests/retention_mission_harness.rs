// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the retention mission integration harness:
//! end-to-end flows through the real runtime.
//!
//! ## Owns
//! - Coding-default resolution tests for new `repo:` banks
//! - The 20-line chatter fixture calibration end to end
//! - Upgrade safety for stored v1 profiles and the `clio retention` CLI surface
//!
//! ## Does not own
//! - Unit-level mission validation and matching (see `clio-config`)
//! - Mission arithmetic unit tests (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to integration behavior or CLI surface should normally require
//! changes here. Changes to unit-level internals should not.
//!
//! Keep this module focused on retention mission integration.

use clio::retention::{RETENTION_PROFILE_VERSION, RetentionProfile, Verbosity};
use clio::{Runtime, ToolResult};
use clio_config::coding_default::CHATTER_FIXTURE;
use clio_config::retention_mission::{MissionEntry, MissionMatchKind, RetentionMission};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// Fresh state whose durable retention store file is `path`, mirroring the
/// env-configured open path (`load_lossy` + configured persist target).
fn state_with_store(path: &Path) -> Arc<McpState> {
    let mut s = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    Arc::get_mut(&mut s)
        .expect("unique state")
        .set_retention_path(path);
    s
}

fn clio_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_clio"))
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn get_profile(state: &McpState, bank: &str) -> Value {
    call(state, "retention_profile_get", json!({"bank": bank}))
}

/// Unique temp path for this run; caller creates and cleans up.
fn temp_dir(tag: &str) -> PathBuf {
    // The counter is what makes this distinct: the process id is constant
    // inside one test binary, so clock-plus-pid alone repeats whenever two
    // calls land in the same tick, and two scenarios then share one
    // directory and read each other's database.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "am-mission-harness-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Gist-only candidate: only the text drives the score.
fn candidate(id: &str, text: &str) -> Value {
    json!({
        "item": {"id": id, "gist": text},
        "episodic_type": "gist",
        "epistemic_kind": "fact"
    })
}

fn chatter_batch() -> Vec<Value> {
    CHATTER_FIXTURE
        .iter()
        .enumerate()
        .map(|(i, line)| candidate(&format!("c{i}"), line))
        .collect()
}

fn preview(state: &McpState, bank: &str, items: &[Value]) -> Value {
    call(
        state,
        "admit_preview_batch",
        json!({"bank": bank, "actor": "agent", "items": items}),
    )
}

/// a new `repo:` bank resolves the coding default (with a mission) and
/// the standard chatter fixture splits 4 keep / 16 `mission_drop`, writing nothing.
#[test]
fn t26_01_coding_default_gates_new_repo_bank_chatter() {
    let dir = temp_dir("proj-a");
    std::fs::create_dir_all(&dir).expect("dir");
    let profiles = dir.join("profiles.json");
    let s = state_with_store(&profiles);

    let bank = "repo:/tmp/am-harness-proj-a";
    let get = get_profile(&s, bank);
    assert_eq!(get["source"], json!("coding_default"), "{get}");
    assert_eq!(get["version"], json!(RETENTION_PROFILE_VERSION));
    assert!(
        get["mission"].is_object(),
        "coding default carries a mission: {get}"
    );
    assert_eq!(get["mission"]["version"], json!(RETENTION_PROFILE_VERSION));

    let out = preview(&s, bank, &chatter_batch());
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(out["profile_source"], json!("coding_default"));
    assert_eq!(out["writes"], json!(0));
    assert_eq!(out["summary"]["would_admit"], json!(4));
    assert_eq!(out["summary"]["would_reject"], json!(16));
    assert_eq!(out["summary"]["by_reason"]["mission_drop"], json!(16));
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), CHATTER_FIXTURE.len());
    for item in items {
        assert!(item["mission_rule_ids"].is_array(), "{item}");
        if item["pass"] == json!(false) {
            let reason = item["rejection_reason"].as_str().expect("reason");
            assert!(
                reason.starts_with("mission_drop:"),
                "chatter must drop on a mission rule: {reason}"
            );
            assert!(
                !item["mission_rule_ids"].as_array().expect("ids").is_empty(),
                "{item}"
            );
        }
    }
    assert!(
        !profiles.exists(),
        "a preview must not create or write the store file"
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Contrast: the same 20 lines under the builtin profile (no mission)
/// admit more than the coding default's 4 keeps.
#[test]
fn t26_02_builtin_bank_without_mission_admits_more_chatter() {
    let s = state();
    let out = preview(&s, "bank-a", &chatter_batch());
    assert_eq!(out["ok"], json!(true), "{out}");
    let admit = out["summary"]["would_admit"].as_u64().expect("admit");
    assert!(
        admit > 4,
        "builtin bank must admit more of the fixture than the coding default: {admit}"
    );
}

/// a stored v1 profile (no mission) stays valid: load converts it to the
/// current version without rewriting the file; sibling `repo:` banks still
/// resolve the coding default, and the Runtime round-trips the file; setting
/// a new mission replaces the whole mission for that bank only.
#[test]
fn t26_03_stored_v1_profile_survives_the_mission_release() {
    let dir = temp_dir("proj-b");
    std::fs::create_dir_all(&dir).expect("dir");
    let profiles = dir.join("profiles.json");
    std::fs::write(
        &profiles,
        r#"{"banks": {"repo:/tmp/am-harness-proj-b": {"verbosity": "selective", "duplicate_tolerance_write": "strict", "version": 1}}}"#,
    )
    .expect("write v1 store");
    let before = std::fs::read_to_string(&profiles).expect("read store");

    let bank = "repo:/tmp/am-harness-proj-b";
    let sibling = "repo:/tmp/am-harness-proj-c";
    let s = state_with_store(&profiles);
    let get = get_profile(&s, bank);
    assert_eq!(get["source"], json!("bank"), "{get}");
    assert_eq!(get["version"], json!(RETENTION_PROFILE_VERSION));
    assert!(
        get["mission"].is_null(),
        "stored v1 profile must keep mission null: {get}"
    );
    let other = get_profile(&s, sibling);
    assert_eq!(other["source"], json!("coding_default"), "{other}");
    assert!(other["mission"].is_object());
    assert_eq!(
        std::fs::read_to_string(&profiles).expect("read store"),
        before,
        "resolution must not rewrite the stored v1 file"
    );

    // The same stored v1 file round-trips through the Runtime surface.
    let mut rt = Runtime::with_env(&|_| None);
    rt.set_retention_path(profiles.clone());
    let ToolResult::Ok { data, .. } = rt.retention_profile_get(bank) else {
        panic!("runtime get");
    };
    assert_eq!(data.source, "bank");
    assert_eq!(data.version, RETENTION_PROFILE_VERSION);
    assert_eq!(data.verbosity, "selective");
    assert_eq!(data.duplicate_tolerance_write, "strict");

    // A new mission profile is stored whole and replaces any prior mission
    // (no merge), for that bank only.
    let mission = |statement: &str, drop: &str| RetentionMission {
        mission_statement: statement.to_owned(),
        keep_examples: Vec::new(),
        drop_examples: vec![MissionEntry {
            text: drop.to_owned(),
            match_kind: MissionMatchKind::Literal,
            reject: true,
        }],
    };
    let first = RetentionProfile {
        verbosity: Verbosity::Selective,
        mission: Some(mission(
            "Keep durable outcomes; drop routine chatter.",
            "tests passed",
        )),
        version: RETENTION_PROFILE_VERSION,
        ..RetentionProfile::default()
    };
    let ToolResult::Ok { data, .. } = rt.retention_profile_set(bank, first) else {
        panic!("runtime set");
    };
    assert!(data.persisted);

    let s2 = state_with_store(&profiles);
    let reloaded = get_profile(&s2, bank);
    assert_eq!(reloaded["source"], json!("bank"), "{reloaded}");
    assert_eq!(reloaded["version"], json!(RETENTION_PROFILE_VERSION));
    assert_eq!(
        reloaded["mission"]["statement"],
        json!("Keep durable outcomes; drop routine chatter.")
    );
    assert_eq!(reloaded["mission"]["drop_examples"], json!(1));

    let second = RetentionProfile {
        verbosity: Verbosity::Selective,
        mission: Some(mission("Keep decisions only.", "heartbeat")),
        version: RETENTION_PROFILE_VERSION,
        ..RetentionProfile::default()
    };
    let ToolResult::Ok { data, .. } = rt.retention_profile_set(bank, second) else {
        panic!("runtime set 2");
    };
    assert!(data.persisted);

    let s3 = state_with_store(&profiles);
    let replaced = get_profile(&s3, bank);
    assert_eq!(
        replaced["mission"]["statement"],
        json!("Keep decisions only."),
        "a new set must replace the whole mission, not merge"
    );
    assert_eq!(replaced["mission"]["drop_examples"], json!(1));
    let still = get_profile(&s3, sibling);
    assert_eq!(still["source"], json!("coding_default"), "{still}");

    let _ = std::fs::remove_dir_all(dir);
}

/// CLI: `clio retention set` stores a mission profile, `get` round-trips
/// the bank profile, and `schema` publishes the mission properties. The schema
/// subcommand needs no `--profiles` flag.
#[test]
fn t26_04_cli_stores_gets_and_schemas_the_mission() {
    let schema = clio_bin()
        .args(["retention", "schema"])
        .output()
        .expect("run schema");
    assert!(schema.status.success());
    let schema_body = String::from_utf8_lossy(&schema.stdout);
    assert!(
        schema_body.contains("\"mission\""),
        "schema properties must contain mission: {schema_body}"
    );
    assert!(schema_body.contains("mission_statement"), "{schema_body}");

    let dir = temp_dir("cli");
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("profiles.json");
    let path_str = path.to_string_lossy().into_owned();
    let bank = "repo:/tmp/am-harness-cli";

    let set = clio_bin()
        .args([
            "retention",
            "set",
            "--bank",
            bank,
            "--profiles",
            &path_str,
            "--profile",
            r#"{"verbosity":"selective","mission":{"mission_statement":"Keep durable outcomes; drop routine chatter.","drop_examples":[{"text":"tests passed"}]}}"#,
        ])
        .output()
        .expect("run set");
    assert!(
        set.status.success(),
        "set failed: {}",
        String::from_utf8_lossy(&set.stderr)
    );
    let set_body = String::from_utf8_lossy(&set.stdout);
    assert!(set_body.contains("\"mission\""), "{set_body}");
    assert!(
        set_body.contains("Keep durable outcomes; drop routine chatter."),
        "{set_body}"
    );

    let get = clio_bin()
        .args(["retention", "get", "--bank", bank, "--profiles", &path_str])
        .output()
        .expect("run get");
    assert!(
        get.status.success(),
        "get failed: {}",
        String::from_utf8_lossy(&get.stderr)
    );
    let get_body = String::from_utf8_lossy(&get.stdout);
    assert!(get_body.contains("\"source\": \"bank\""), "{get_body}");
    assert!(get_body.contains("\"version\": 3"), "{get_body}");
    assert!(
        get_body.contains("\"verbosity\": \"selective\""),
        "{get_body}"
    );

    let _ = std::fs::remove_dir_all(dir);
}
