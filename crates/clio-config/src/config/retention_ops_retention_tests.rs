// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the retention profile Runtime handlers.
//!
//! ## Owns
//! - Profile get/set round-trip, persistence, and error-path scenarios
//!
//! ## Does not own
//! - Production runtime logic under test
//! - Profile object/mapping tests (see `crate::retention`)
//!
//! ## Boundary
//!
//! Changes to expected retention tool behavior under test should normally require changes here.
//! Changes to unrelated config tools should not require changes here.
//!
//! Keep this module focused on retention profile Runtime handler tests.

use crate::config::*;
use crate::error::{ErrorCode, ToolResult};
use crate::retention::{RETENTION_PROFILE_VERSION, RetentionProfile, Verbosity};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn retention_profile_round_trips_across_a_restart() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("am-retention-restart-{nanos}.json"));

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
    assert_eq!(data.scope, "bank");

    // A fresh runtime pointed at the same file re-reads the stored profile.
    let mut restarted = Runtime::with_env(&|_| None);
    restarted.set_retention_path(path.clone());
    let ToolResult::Ok { data, .. } = restarted.retention_profile_get("bank-a") else {
        panic!("get");
    };
    assert_eq!(data.source, "bank");
    assert_eq!(data.verbosity, "selective");
    assert_eq!(data.version, RETENTION_PROFILE_VERSION);
    assert_eq!(
        data.accepted_but_inert["honored_by"],
        "recall_scope_and_dedup"
    );

    // A bank with no override resolves to the builtin default.
    let ToolResult::Ok { data, .. } = restarted.retention_profile_get("bank-b") else {
        panic!("get b");
    };
    assert_eq!(data.source, "builtin");
    assert_eq!(data.verbosity, "balanced");

    // Deployment default applies to banks without an override.
    let ToolResult::Ok { data, .. } =
        restarted.retention_profile_set_deployment(RetentionProfile {
            verbosity: Verbosity::Selective,
            ..RetentionProfile::default()
        })
    else {
        panic!("deploy");
    };
    assert_eq!(data.scope, "deployment");
    assert!(data.bank.is_none());
    let ToolResult::Ok { data, .. } = restarted.retention_profile_get("bank-b") else {
        panic!("get b2");
    };
    assert_eq!(data.source, "deployment");
    assert_eq!(data.verbosity, "selective");

    // `permissive` MUST NOT become a deployment default.
    let ToolResult::Err { code, .. } =
        restarted.retention_profile_set_deployment(RetentionProfile {
            verbosity: Verbosity::Permissive,
            ..RetentionProfile::default()
        })
    else {
        panic!("permissive deployment must be rejected");
    };
    assert_eq!(code, ErrorCode::InvalidArgument);
    let ToolResult::Ok { data, .. } = restarted.retention_profile_get("bank-b") else {
        panic!("get b3");
    };
    assert_eq!(data.source, "deployment");
    assert_eq!(
        data.verbosity, "selective",
        "rejected default must not apply"
    );

    // Reload keeps the store readable.
    restarted.reload_retention();
    assert!(matches!(
        restarted.retention_profile_get("bank-a"),
        ToolResult::Ok { .. }
    ));

    let _ = std::fs::remove_file(path);
}

#[test]
fn retention_profile_errors_and_in_memory_only_paths() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.clear_retention_path();

    let ToolResult::Err { code, .. } = rt.retention_profile_get("") else {
        panic!("empty bank");
    };
    assert_eq!(code, ErrorCode::MissingBank);

    let ToolResult::Err { code, .. } = rt.retention_profile_set(
        "bank-a",
        RetentionProfile {
            theta_offsets: std::collections::BTreeMap::from([("nope".to_owned(), 0.1)]),
            ..RetentionProfile::default()
        },
    ) else {
        panic!("bad offset");
    };
    assert_eq!(code, ErrorCode::UnknownPath);

    // No store path configured: the write succeeds in memory and reports no persistence.
    let ToolResult::Ok { data, .. } =
        rt.retention_profile_set("bank-a", RetentionProfile::default())
    else {
        panic!("in-memory set");
    };
    assert!(!data.persisted);

    // A write to an unwritable store path surfaces the I/O failure.
    let dir = std::env::temp_dir().join(format!(
        "am-retention-dir-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("t")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let mut broken = Runtime::with_env(&|_| None);
    broken.set_retention_path(dir.clone());
    let ToolResult::Err { code, .. } =
        broken.retention_profile_set("bank-a", RetentionProfile::default())
    else {
        panic!("expected save error");
    };
    assert_eq!(code, ErrorCode::Internal);
    // A failed persist rolls back: the in-memory store holds no new override.
    let ToolResult::Ok { data, .. } = broken.retention_profile_get("bank-a") else {
        panic!("get after failed persist");
    };
    assert_eq!(data.source, "builtin");
    let _ = std::fs::remove_dir_all(dir);
}

/// A request that omits the version field reads the current default, and every
/// other omitted patch field keeps its documented default too.
#[test]
fn a_partial_request_omitting_version_keeps_the_current_defaults() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.clear_retention_path();
    let set = rt.call_tool(
        "retention_profile_set",
        &serde_json::json!({"bank": "bank-a", "profile": {"verbosity": "selective"}}),
    );
    assert_eq!(set["ok"], serde_json::json!(true), "{set}");

    let get = rt.call_tool(
        "retention_profile_get",
        &serde_json::json!({"bank": "bank-a"}),
    );
    assert_eq!(get["source"], serde_json::json!("bank"));
    assert_eq!(get["version"], serde_json::json!(RETENTION_PROFILE_VERSION));
    assert_eq!(get["verbosity"], serde_json::json!("selective"));
    assert_eq!(
        get["duplicate_tolerance_write"],
        serde_json::json!("balanced"),
        "an omitted patch field keeps its documented default"
    );
    assert_eq!(get["theta_offsets"], serde_json::json!({}));
}

/// A request that carries an older version marker is refused with the retention
/// version error; only a stored file is converted.
#[test]
fn a_request_carrying_an_older_version_is_refused() {
    let mut rt = Runtime::with_env(&|_| None);
    // An isolated, unreadable store path loads as an empty store, so the
    // operator's profiles file is never read or written here.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    rt.set_retention_path(std::env::temp_dir().join(format!("am-retention-refused-{nanos}.json")));
    let out = rt.call_tool(
        "retention_profile_set",
        &serde_json::json!({
            "bank": "bank-a",
            "profile": {"verbosity": "selective", "version": 2}
        }),
    );
    assert_eq!(out["ok"], serde_json::json!(false), "{out}");
    assert_eq!(out["code"], serde_json::json!("out_of_range"), "{out}");
    assert!(
        out["message"]
            .as_str()
            .expect("message")
            .contains(&format!("expected {RETENTION_PROFILE_VERSION}")),
        "{out}"
    );
    // The refused request stored nothing.
    let get = rt.call_tool(
        "retention_profile_get",
        &serde_json::json!({"bank": "bank-a"}),
    );
    assert_eq!(get["source"], serde_json::json!("builtin"));
}
