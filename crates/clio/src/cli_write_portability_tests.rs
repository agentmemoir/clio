// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` portability write commands: the export→import
//! round trip and its idempotency, the dry-run and confirmation gates for
//! erasure, usage fail-closed paths, the privileged-actor gate, and secret
//! masking in export output.
//!
//! ## Owns
//! - Behavior, golden, and group-contract tests for `cli_write_portability`
//!
//! ## Does not own
//! - The command under test (`cli_write_portability`)
//! - Bundle, import, and erasure semantics (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{json_of, seed_item, state};
use crate::cli_write_group::WriteGroup;
use crate::cli_write_test_support::{bank_count, journal_state, run_write_state};
use serde_json::{Value, json};
use std::process::ExitCode;

/// A temp bundle path unique to `label` within this test process.
fn temp_bundle(label: &str) -> String {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "clio-portability-{label}-{}.json",
        std::process::id()
    ));
    path.to_string_lossy().into_owned()
}

// ---------- export -> import round trip, idempotent ----------

#[test]
fn export_import_round_trip_is_idempotent() {
    let s = state();
    seed_item(&s, "demo", "itm-rt1", "prefers terse answers");
    seed_item(&s, "demo", "itm-rt2", "prefers aisle seats");
    let dest = temp_bundle("roundtrip");
    let (code, out, err) = run_write_state(
        "export",
        &["--out", &dest, "--actor", "harness", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["manifest"]["complete"], json!(true), "{payload}");
    let raw = std::fs::read_to_string(&dest).expect("bundle file exists");
    let bundle: Value = serde_json::from_str(&raw).expect("bundle parses as JSON");
    assert_eq!(bundle["manifest"]["complete"], json!(true), "{bundle}");

    // A fresh state restores into ctx.bank (`demo`).
    let fresh = state();
    let (code, out, err) = run_write_state(
        "import",
        &["--source", &dest, "--bank", "demo"],
        &fresh,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let report = json_of(&out)["report"].clone();
    assert_eq!(report["applied"], json!(true), "{report}");
    assert!(
        report["would_create"].as_u64().unwrap_or(0) >= 2,
        "{report}"
    );

    // The SAME bundle again on the same state: everything skips.
    let (code, out, err) = run_write_state(
        "import",
        &["--source", &dest, "--bank", "demo"],
        &fresh,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let report = &json_of(&out)["report"];
    assert_eq!(report["would_create"], json!(0), "{report}");
    assert!(report["would_skip"].as_u64().unwrap_or(0) >= 2, "{report}");
    let _ = std::fs::remove_file(&dest);
}

// ---------- import --dry-run proves zero writes ----------

#[test]
fn import_dry_run_reports_without_writing() {
    let s = state();
    seed_item(&s, "demo", "itm-dr1", "prefers terse answers");
    seed_item(&s, "demo", "itm-dr2", "prefers aisle seats");
    let dest = temp_bundle("dryrun");
    let (code, out, err) = run_write_state(
        "export",
        &["--out", &dest, "--actor", "harness", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let before = bank_count(&s, "demo");

    let (code, out, err) = run_write_state(
        "import",
        &["--source", &dest, "--bank", "demo", "--dry-run"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let report = json_of(&out)["report"].clone();
    assert_eq!(report["dry_run"], json!(true), "{report}");
    assert_eq!(report["would_create"], json!(0), "existing state skips");
    assert!(report["would_skip"].as_u64().unwrap_or(0) >= 2, "{report}");
    assert_eq!(bank_count(&s, "demo"), before, "dry-run must write nothing");

    let (code, text, err) = run_write_state(
        "import",
        &["--source", &dest, "--bank", "demo", "--dry-run"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{text}{err}");
    assert!(
        text.contains("import dry-run: would create 0, skip 2, reject 0 (zero writes)"),
        "{text}"
    );
    let _ = std::fs::remove_file(&dest);
}

// ---------- erasure gates ----------

#[test]
fn erase_without_confirm_refuses_with_exit_two() {
    let s = state();
    seed_item(&s, "demo", "itm-nc", "prefers terse answers");
    let before = bank_count(&s, "demo");
    let (code, out, err) = run_write_state(
        "erase",
        &[
            "--subject",
            "s",
            "--basis",
            "B",
            "--request",
            "R",
            "--actor",
            "harness",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--confirm"), "{err}");
    assert_eq!(
        bank_count(&s, "demo"),
        before,
        "refusal must be zero writes"
    );
}

#[test]
fn erase_with_confirm_completes_the_shred() {
    let (journal, s) = journal_state("erase-confirm");
    seed_item(&s, "demo", "itm-e1", "prefers terse answers");
    seed_item(&s, "demo", "itm-e2", "prefers aisle seats");
    let (code, out, err) = run_write_state(
        "erase",
        &[
            "--subject",
            "s",
            "--basis",
            "gdpr-art-17",
            "--request",
            "req-1",
            "--confirm",
            "--actor",
            "harness",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    let status = payload["status"].as_str().expect("status");
    assert!(status == "COMPLETED" || status == "PROCESSING", "{payload}");
    assert!(payload["item_count"].as_u64().unwrap_or(0) > 0, "{payload}");
    assert!(
        payload["tombstones_written"].as_u64().unwrap_or(0) > 0,
        "{payload}"
    );
    assert!(
        journal.path().exists(),
        "the confirmed erase must record its denial at the injected journal path"
    );
}

#[test]
fn erase_dry_run_previews_irreversibility_with_zero_writes() {
    let s = state();
    seed_item(&s, "demo", "itm-prev", "prefers terse answers");
    let before = bank_count(&s, "demo");
    let args = &[
        "--subject",
        "s",
        "--basis",
        "B",
        "--request",
        "R",
        "--dry-run",
        "--actor",
        "harness",
        "--bank",
        "demo",
    ];
    let (code, out, err) = run_write_state("erase", args, &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["irreversible"], json!(true), "{payload}");
    assert_eq!(payload["operation"], json!("erase"), "{payload}");
    assert_eq!(bank_count(&s, "demo"), before, "preview must write nothing");

    let (code, text, err) = run_write_state("erase", args, &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{text}{err}");
    assert!(text.contains("IRREVERSIBLE"), "{text}");
    assert!(text.contains("discard"), "distinct from discard: {text}");
    assert!(text.contains("hygiene"), "distinct from hygiene: {text}");
}

// ---------- fail-closed paths ----------

#[test]
fn export_as_default_agent_actor_is_forbidden_without_a_file() {
    let s = state();
    let dest = temp_bundle("forbidden");
    let (code, out, err) =
        run_write_state("export", &["--out", &dest, "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(
        err.contains("harness or system actor") || err.contains("forbidden"),
        "the tool's Forbidden gate must surface: {out}{err}"
    );
    assert!(
        !std::path::Path::new(&dest).exists(),
        "no file may be created"
    );
}

#[test]
fn usage_failures_exit_two_and_name_the_missing_flag() {
    let s = state();
    let dest = temp_bundle("usage");

    let (code, out, err) = run_write_state("export", &["--actor", "harness"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--out"), "{err}");

    let (code, out, err) = run_write_state("import", &[], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--source"), "{err}");

    for missing in [
        vec!["--basis", "B", "--request", "R"],
        vec!["--subject", "s", "--request", "R"],
        vec!["--subject", "s", "--basis", "B"],
    ] {
        let (code, out, err) = run_write_state("erase", &missing, &s, false);
        assert_eq!(code, ExitCode::from(2), "{missing:?}: {out}{err}");
    }

    let (code, out, err) = run_write_state(
        "export",
        &[
            "--out",
            &dest,
            "--content-mode",
            "bogus",
            "--actor",
            "harness",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("content-mode"), "{err}");

    let (code, out, err) = run_write_state(
        "export",
        &[
            "--out",
            &dest,
            "--include-archived",
            "bogus",
            "--actor",
            "harness",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("include-archived"), "{err}");
}

// ---------- masking ----------

#[test]
fn export_keeps_planted_secrets_out_of_the_cli_output() {
    let s = state();
    let secret = "sk-supersecret-9876";
    seed_item(&s, "demo", "itm-secret", &format!("api_key: {secret}"));
    let dest = temp_bundle("masking");
    let (code, out, err) = run_write_state(
        "export",
        &["--out", &dest, "--actor", "harness", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert!(!out.contains(secret), "stdout leaked the secret: {out}");
    assert!(!err.contains(secret), "stderr leaked the secret: {err}");
    let _ = std::fs::remove_file(&dest);
}

// ---------- group contract / render goldens ----------

#[test]
fn group_contract_holds_for_every_verb() {
    let group = super::PortabilityWriteGroup;
    assert_eq!(group.verbs(), &["export", "import", "erase"]);
    assert!(group.mutates("import"), "import must drain the index");
    assert!(!group.mutates("export"));
    assert!(!group.mutates("erase"));
    assert!(group.refusal("export", &json!({"ok": true})).is_none());
    assert!(group.refusal("erase", &json!({})).is_none());

    let export_spec = group.spec("export");
    for flag in ["out", "content-mode", "include-archived"] {
        assert!(export_spec.value_flags().contains(&flag), "{flag}");
    }
    let import_spec = group.spec("import");
    assert!(import_spec.bool_flags().contains(&"dry-run"));
    assert!(import_spec.bool_flags().contains(&"force"));
    let erase_spec = group.spec("erase");
    assert!(erase_spec.bool_flags().contains(&"confirm"));
    assert!(erase_spec.value_flags().contains(&"reason-code"));
}

#[test]
fn build_erase_dry_run_returns_the_zero_write_preview() {
    let group = super::PortabilityWriteGroup;
    let parsed = crate::cli_args::parse(
        &group.spec("erase"),
        crate::cli_read_test_support::tokens(&[
            "--subject",
            "s",
            "--basis",
            "B",
            "--request",
            "R",
            "--dry-run",
        ]),
    )
    .expect("parse");
    match group.build("erase", &parsed).expect("preview") {
        crate::cli_write_group::WriteCall::Preview { payload, text } => {
            assert_eq!(payload["dry_run"], json!(true), "{payload}");
            assert_eq!(payload["irreversible"], json!(true), "{payload}");
            assert_eq!(payload["subject"], json!("s"), "{payload}");
            assert!(text.contains("IRREVERSIBLE"), "{text}");
            assert!(text.contains("hygiene"), "{text}");
        }
        crate::cli_write_group::WriteCall::Tool { .. } => {
            panic!("dry-run must build a preview, not a dispatcher call")
        }
    }
}

#[test]
fn render_matches_the_contract_for_each_verb() {
    let group = super::PortabilityWriteGroup;
    let export = group.render(
        "export",
        &json!({
            "ok": true,
            "destination": "/tmp/bundle.json",
            "content_mode": "dsar_plaintext",
            "manifest": {
                "content_mode": "dsar_plaintext",
                "complete": true,
                "counts_by_type": [["items", 2], ["triples", 3]],
                "content_sha256": "abcdef1234567890"
            }
        }),
        crate::cli_style::Palette::new(false),
    );
    assert_eq!(
        export,
        "export wrote /tmp/bundle.json (content_mode=dsar_plaintext, complete=true, \
         entities=5, sha256=abcdef123456…)"
    );

    let dry = group.render(
        "import",
        &json!({"ok": true, "report": {"dry_run": true, "would_create": 2, "would_skip": 1, "would_reject": 0}}),
        crate::cli_style::Palette::new(false),
    );
    assert_eq!(
        dry,
        "import dry-run: would create 2, skip 1, reject 0 (zero writes)"
    );

    let live = group.render(
        "import",
        &json!({"ok": true, "report": {"dry_run": false, "applied": true, "would_create": 2,
                 "would_skip": 1, "would_reject": 0, "would_overwrite": 4}}),
        crate::cli_style::Palette::new(false),
    );
    assert_eq!(
        live,
        "import: created 2, skipped 1, rejected 0, overwrote 4"
    );

    let not_applied = group.render(
        "import",
        &json!({"ok": true, "report": {"dry_run": false, "applied": false, "would_create": 0,
                 "would_skip": 2, "would_reject": 0, "would_overwrite": 0}}),
        crate::cli_style::Palette::new(false),
    );
    assert!(not_applied.contains("applied nothing"), "{not_applied}");

    let erased = group.render(
        "erase",
        &json!({"status": "COMPLETED", "request_id": "req-9", "item_count": 2,
                "triples_purged": 1, "tombstones_written": 2}),
        crate::cli_style::Palette::new(false),
    );
    assert_eq!(
        erased,
        "erased 2 item(s) for request req-9 (status COMPLETED; DEK destroyed; \
         triples 1; tombstones 2; irreversible)"
    );
}
