// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore surface's golden outputs: the
//! full text blast radius, the full text report, the full JSON payload, and a
//! full text-mode not-found error, pinned byte for byte against a hand-built archive
//! whose id and stamps are fixed.
//!
//! ## Owns
//! - The four golden comparisons and the scratch-path normalization they use
//! - The one-item store's singular counts, which a three-item golden cannot catch
//!
//! ## Does not own
//! - The restore surface itself (see `backup_restore_cli`)
//! - The hand-built archive fixture (see `backup_cli_fixtures_handmade`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Only the scratch
//! directory path is normalized; every other byte of every output is pinned.

use super::*;
use crate::backup_cli_fixtures::{Scratch, key_store_path, populated_store, run_in, run_json_in};
use crate::backup_cli_fixtures_handmade::hand_built_archive;
use std::path::PathBuf;

/// The golden archive's identity: fixed, so every output byte is stable. The
/// manifest stamp the fixture writes is `2026-09-27T00:00:00Z`.
const GOLDEN_ID: &str = "clio-20260927T000000Z-golden01";

/// The default target store inside `scratch`.
fn store_path(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// The live key store beside that target, which is where a restore places the
/// archive's key member.
fn kms_path(scratch: &Scratch) -> PathBuf {
    key_store_path(&store_path(scratch))
}

/// The golden setup: a populated store, its key store, and the fixed archive.
fn golden_scratch(tag: &str) -> (Scratch, String) {
    let scratch = Scratch::new(tag);
    populated_store(&store_path(&scratch), 3, 1);
    let (id, _) = hand_built_archive(&scratch, &store_path(&scratch), GOLDEN_ID, false, |_| ());
    (scratch, id)
}

/// The scratch directory's own path, which varies per run; the goldens name
/// it `<scratch>` so every other byte can be pinned.
fn scratch_token(scratch: &Scratch) -> String {
    scratch.path().display().to_string()
}

#[test]
fn the_text_plan_and_report_are_pinned_byte_for_byte() {
    let (scratch, id) = golden_scratch("restore-golden-text");
    let token = scratch_token(&scratch);

    let (code, out, err) = run_in(
        &scratch,
        &[
            "restore",
            &id,
            "--yes",
            "--no-pre-backup",
            "--output",
            "text",
        ],
        false,
    );

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let out = out.replace(&token, "<scratch>");
    let err = err.replace(&token, "<scratch>");
    assert_eq!(
        err,
        concat!(
            "restore plan: snapshot clio-20260927T000000Z-golden01 (created 2026-09-27T00:00:00Z)\n",
            "  target <scratch>/clio.db (sqlite, clio.db)\n",
            "  current store: 3 items, 0 triples, 42 tables, banks: demo\n",
            "  incoming archive: 3 items, 0 triples, 42 tables, banks: demo\n",
            "  schema: archive 1 vs build 1 - none\n",
            "  the store is replaced, not merged\n",
            "  the key store at <scratch>/clio.db.kms.json is replaced too\n",
            "  pre-restore snapshot: skipped (--no-pre-backup was given)\n",
        ),
        "the plan"
    );
    assert_eq!(
        out,
        concat!(
            "restore clio-20260927T000000Z-golden01 over <scratch>/clio.db (sqlite, clio.db)\n",
            "  schema: archive 1 vs build 1 - none\n",
            "  counts before: 3 items, 0 triples, 42 tables, banks: demo\n",
            "  counts after: 3 items, 0 triples, 42 tables, banks: demo\n",
            "  the store is replaced, not merged\n",
            "  key store: <scratch>/clio.db.kms.json was replaced\n",
            "  pre-restore snapshot: skipped (--no-pre-backup was given)\n",
            "  result: restored\n",
        ),
        "the report"
    );
}

#[test]
fn the_json_payload_is_pinned_field_for_field() {
    let (scratch, id) = golden_scratch("restore-golden-json");
    let path = store_path(&scratch).display().to_string();

    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--no-pre-backup"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload,
        serde_json::json!({
            "ok": true,
            "snapshot_id": GOLDEN_ID,
            "target": {"path": path, "backend": "sqlite", "database": "clio.db", "schema": null},
            "pre_restore_snapshot": {
                "id": null,
                "path": null,
                "skipped": true,
                "reason": "--no-pre-backup was given",
            },
            "counts": {
                "before": {"items": 3, "triples": 0, "tables": 42, "banks": ["demo"]},
                "after": {"items": 3, "triples": 0, "tables": 42, "banks": ["demo"]},
                "incoming": null,
            },
            "schema": {"archive": "1", "build": clio_store::SCHEMA_VERSION, "action": "none"},
            "would_replace": [],
            "key_store": {
                "in_archive": true,
                "replaced": true,
                "path": kms_path(&scratch).display().to_string(),
            },
            "result": "restored",
            "failed_step": null,
            "failure": null,
            "dry_run": false,
            "directory_synced": true,
        }),
        "{payload}"
    );
}

/// The goldens use a store of three, so no golden could catch a counts phrase
/// that has no singular form. This one holds exactly one item, and pins the
/// singular nouns on the plan's line and the report's line.
#[test]
fn a_one_item_store_prints_singular_counts() {
    let scratch = Scratch::new("restore-golden-singular");
    let store = store_path(&scratch);
    populated_store(&store, 1, 1);
    let (id, _) = hand_built_archive(&scratch, &store, GOLDEN_ID, false, |_| ());
    let token = scratch_token(&scratch);

    let (code, out, err) = run_in(
        &scratch,
        &[
            "restore",
            &id,
            "--yes",
            "--no-pre-backup",
            "--output",
            "text",
        ],
        false,
    );

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let out = out.replace(&token, "<scratch>");
    let err = err.replace(&token, "<scratch>");
    assert!(
        err.contains("current store: 1 item, 0 triples, 42 tables, banks: demo"),
        "the plan's counts line: {err}"
    );
    assert!(
        out.contains("counts before: 1 item, 0 triples, 42 tables, banks: demo"),
        "the report's counts line: {out}"
    );
}

#[test]
fn text_mode_missing_snapshot_is_pinned_byte_for_byte() {
    let (scratch, _) = golden_scratch("restore-golden-refusal");

    let (code, out, err) = run_in(&scratch, &["restore", "zzzz", "--output", "text"], false);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert_eq!(out, "", "a refusal prints nothing to stdout");
    assert_eq!(
        err,
        concat!(
            "error: no snapshot `zzzz` in the backup directory\n",
            "hint: run `clio backup list` to see the snapshots\n",
        ),
        "the not-found error"
    );

    let (code, _, err) = run_json_in(&scratch, &["restore", "zzzz"]);
    assert_eq!(code, ExitCode::from(1), "{err}");
    let error: serde_json::Value = serde_json::from_str(err.trim()).expect("error envelope");
    assert_eq!(error["ok"], serde_json::json!(false), "{error}");
    assert_eq!(error["code"], serde_json::json!("not_found"), "{error}");
}
