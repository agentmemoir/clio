// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio hygiene clean` command: dry-run counts with
//! zero writes, confirmed apply, the `delete` non-compliance alias, stdin
//! candidates, fail-closed file decoding, and action validation.
//!
//! ## Owns
//! - Behavior tests for the `hygiene clean` write verb
//!
//! ## Does not own
//! - The command implementation under test (`cli_write_hygiene`)
//! - Clean semantics or the dry-run decision (see `clio-hygiene`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{HygieneWriteGroup, parse_candidates};
use crate::cli_read_test_support::{call, json_of, seed_item, state};
use crate::cli_write_group::WriteGroup;
use crate::cli_write_test_support::{run_write_state, run_write_state_stdin};
use serde_json::json;
use std::io::Cursor;
use std::process::ExitCode;

/// Write a candidates JSON file under the process-unique temp name.
fn candidates_file(name: &str, content: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("clio-hygiene-{name}-{}.json", std::process::id()));
    std::fs::write(&path, content).expect("write candidates file");
    path
}

/// The `stats` counts for `demo`, used to prove writes did or did not happen.
fn stats(s: &clio_mcp::runtime::McpState) -> serde_json::Value {
    let report = call(s, "stats", json!({"bank": "demo"}));
    assert_eq!(report["ok"], json!(true), "{report}");
    report
}

/// One item in `demo` plus a candidates file that names it.
fn seeded_with_candidates(
    name: &str,
    id: &str,
) -> (
    std::sync::Arc<clio_mcp::runtime::McpState>,
    std::path::PathBuf,
) {
    let s = state();
    seed_item(&s, "demo", id, "stale duplicated note");
    let candidate = json!({"item_id": id, "noise_score": 0.9, "suggested_action": "discard"});
    let path = candidates_file(name, &json!({"candidates": [candidate]}).to_string());
    (s, path)
}

#[test]
fn clean_dry_run_counts_without_writing_anything() {
    let (s, path) = seeded_with_candidates("dry", "itm-h1");
    let file = format!("--file={}", path.display());
    let (code, out, err) = run_write_state(
        "hygiene clean",
        &[&file, "--action", "discard", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["would_discard"], json!(1), "{payload}");
    assert_eq!(stats(&s)["items_active"], json!(1), "dry-run: zero writes");
    let (_, text, _) = run_write_state(
        "hygiene clean",
        &[&file, "--action", "discard", "--bank", "demo"],
        &s,
        true,
    );
    assert!(
        text.contains(
            "hygiene clean dry-run: would flag 0, archive 0, discard 1 \
             (zero writes; pass --confirm to apply)"
        ),
        "{text}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn confirmed_clean_discards_and_reports_the_applied_ids() {
    let (s, path) = seeded_with_candidates("confirm", "itm-h2");
    let file = format!("--file={}", path.display());
    let (code, out, err) = run_write_state(
        "hygiene clean",
        &[
            &file,
            "--action",
            "discard",
            "--confirm",
            "--reason",
            "noise",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(false), "{payload}");
    assert_eq!(payload["applied"], json!(["itm-h2"]), "{payload}");
    // The confirmed JSON keeps the counter meaning: would_discard equals the
    // applied discard count, as the preview reported.
    assert_eq!(payload["would_discard"], json!(1), "{payload}");
    assert_eq!(payload["would_flag"], json!(0), "{payload}");
    assert_eq!(payload["would_archive"], json!(0), "{payload}");
    assert_eq!(stats(&s)["items_active"], json!(0), "item discarded");
    assert_eq!(
        super::render_clean(&json!({"dry_run": false, "applied": ["a", "b"], "pending": []})),
        "hygiene clean: applied 2 item(s)"
    );
    assert_eq!(
        super::render_clean(&json!({"dry_run": false, "applied": ["a"], "pending": ["b"]})),
        "hygiene clean: applied 1 item(s); aborted, 1 pending"
    );
    // A batch abort never hides the failing target.
    assert_eq!(
        super::render_clean(&json!({
            "dry_run": false, "applied": ["a"], "pending": ["c"],
            "failed": {"item_id": "b", "error": "not_found: gone"}
        })),
        "hygiene clean: applied 1 item(s); failed b (not_found: gone); aborted, 1 pending"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_delete_alias_is_a_non_compliance_discard() {
    let (s, path) = seeded_with_candidates("alias", "itm-h3");
    let file = format!("--file={}", path.display());
    let (code, out, err) = run_write_state(
        "hygiene clean",
        &[&file, "--action", "delete", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["would_discard"], json!(1), "{out}");

    let (code, out, err) = run_write_state(
        "hygiene clean",
        &[
            &file,
            "--action",
            "delete",
            "--confirm",
            "--reason",
            "noise",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["applied"], json!(["itm-h3"]), "{out}");
    assert_eq!(json_of(&out)["would_discard"], json!(1), "{out}");
    assert_eq!(stats(&s)["items_active"], json!(0), "alias discarded");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn clean_reads_candidates_from_stdin_with_dash() {
    let s = state();
    seed_item(&s, "demo", "itm-h4", "stale duplicated note");
    let document = json!({"candidates": [{"item_id": "itm-h4"}]}).to_string();
    let mut reader = Cursor::new(document.into_bytes());
    let (code, out, err) = run_write_state_stdin(
        "hygiene clean",
        &["--file", "-", "--action", "flag"],
        &mut reader,
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["would_flag"], json!(1), "{out}");
    assert_eq!(stats(&s)["items_active"], json!(1), "dry-run: zero writes");
}

#[test]
fn clean_accepts_the_bare_candidates_array_shape() {
    let s = state();
    seed_item(&s, "demo", "itm-h5", "stale duplicated note");
    let path = candidates_file("bare", &json!([{"item_id": "itm-h5"}]).to_string());
    let file = format!("--file={}", path.display());
    let (code, out, err) =
        run_write_state("hygiene clean", &[&file, "--action", "archive"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["would_archive"], json!(1), "{out}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn clean_file_failures_exit_two_before_any_dispatcher_call() {
    let s = state();
    seed_item(&s, "demo", "itm-h6", "stale duplicated note");
    let non_json = candidates_file("bad", "{not json");
    let no_key = candidates_file(
        "nokey",
        &json!({"items": [{"item_id": "itm-h6"}]}).to_string(),
    );
    let no_id = candidates_file("noid", &json!([{"noise_score": 0.5}]).to_string());
    let wrong_types = candidates_file(
        "types",
        &json!({"candidates": [{"item_id": "itm-h6", "noise_score": "high"}]}).to_string(),
    );
    let cases: Vec<(Vec<String>, &str)> = vec![
        (
            vec!["--file=/nonexistent/candidates.json".to_owned()],
            "candidates.json",
        ),
        (vec!["--file=-".into()], "JSON"),
        (vec![format!("--file={}", non_json.display())], "JSON"),
        (vec![format!("--file={}", no_key.display())], "candidates"),
        (vec![format!("--file={}", no_id.display())], "item_id"),
        (
            vec![format!("--file={}", wrong_types.display())],
            "noise_score",
        ),
    ];
    for (args, fragment) in &cases {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let (code, out, err) = run_write_state("hygiene clean", &refs, &s, false);
        assert_eq!(code, ExitCode::from(2), "{out}{err}");
        assert!(err.contains(fragment), "{err}");
    }
    assert_eq!(
        stats(&s)["items_active"],
        json!(1),
        "fail closed: zero writes"
    );
    for path in [non_json, no_key, no_id, wrong_types] {
        let _ = std::fs::remove_file(&path);
    }
}

#[test]
fn clean_rejects_unknown_actions_and_missing_files() {
    let (s, path) = seeded_with_candidates("action", "itm-h7");
    let file = format!("--file={}", path.display());
    let (code, out, err) =
        run_write_state("hygiene clean", &[&file, "--action", "erase"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("flag|archive|discard|delete|keep"), "{err}");

    let (code, out, err) = run_write_state("hygiene clean", &[], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--file"), "{err}");
    assert_eq!(stats(&s)["items_active"], json!(1), "zero writes");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn clean_reports_a_confirmed_batch_abort_in_text() {
    let (s, path) = seeded_with_candidates("abort", "itm-h8");
    // Only itm-h8 exists; the next candidate id is a ghost that aborts the
    // batch, leaving the trailing target pending (never attempted).
    let candidates = json!({
        "candidates": [
            {"item_id": "itm-h8"},
            {"item_id": "itm-ghost"},
            {"item_id": "itm-later"}
        ]
    })
    .to_string();
    std::fs::write(&path, candidates).expect("rewrite candidates");
    let file = format!("--file={}", path.display());
    let (code, out, err) = run_write_state(
        "hygiene clean",
        &[&file, "--action", "flag", "--confirm", "--bank", "demo"],
        &s,
        true,
    );
    // A confirmed batch abort is a partial failure: exit 1, with the failing
    // target and the pending remainder visible in the text view.
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(out.contains("applied 1 item(s)"), "{out}");
    assert!(out.contains("failed itm-ghost"), "{out}");
    assert!(out.contains("aborted, 1 pending"), "{out}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn clean_confirmed_batch_abort_exits_one_with_json_detail() {
    let (s, path) = seeded_with_candidates("abort-json", "itm-j1");
    let candidates = json!({
        "candidates": [
            {"item_id": "itm-j1"},
            {"item_id": "itm-jghost"},
            {"item_id": "itm-jlater"}
        ]
    })
    .to_string();
    std::fs::write(&path, candidates).expect("rewrite candidates");
    let file = format!("--file={}", path.display());
    let (code, out, err) = run_write_state(
        "hygiene clean",
        &[&file, "--action", "flag", "--confirm", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["applied"], json!(["itm-j1"]), "{payload}");
    assert_eq!(
        payload["failed"]["item_id"],
        json!("itm-jghost"),
        "{payload}"
    );
    assert_eq!(payload["pending"], json!(["itm-jlater"]), "{payload}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn clean_dry_run_flag_forces_zero_writes_even_with_confirm() {
    let (s, path) = seeded_with_candidates("dryflag", "itm-d1");
    let file = format!("--file={}", path.display());
    let (code, out, err) = run_write_state(
        "hygiene clean",
        &[
            &file,
            "--action",
            "discard",
            "--confirm",
            "--dry-run",
            "--reason",
            "noise",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["would_discard"], json!(1), "{payload}");
    assert_eq!(
        stats(&s)["items_active"],
        json!(1),
        "--dry-run: zero writes"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------- build/parse unit tests ----------

#[test]
fn build_without_streams_rejects_the_stdin_path() {
    let parsed = crate::cli_args::parse(
        &HygieneWriteGroup.spec("hygiene clean"),
        ["--file".to_owned(), "-".to_owned()],
    )
    .expect("parse");
    let Err(error) = HygieneWriteGroup.build("hygiene clean", &parsed) else {
        panic!("no stdin on this path")
    };
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("stdin"), "{}", error.message());
}

#[test]
fn parse_candidates_accepts_both_document_shapes() {
    let object = r#"{"candidates": [{"item_id": "a"}], "bank": "demo"}"#;
    assert_eq!(parse_candidates(object).unwrap().len(), 1);
    let bare = r#"[{"item_id": "a"}, {"item_id": "b", "noise_score": 0.5}]"#;
    assert_eq!(parse_candidates(bare).unwrap().len(), 2);
}

#[test]
fn parse_candidates_passes_optional_fields_and_drops_extras() {
    let raw = r#"[{"item_id": "a", "noise_score": 0.9, "suggested_action": "discard",
                   "noise_reasons": ["x"], "preview": "leak"}]"#;
    let candidates = parse_candidates(raw).unwrap();
    assert_eq!(candidates[0]["item_id"], json!("a"));
    assert_eq!(candidates[0]["noise_score"], json!(0.9));
    assert_eq!(candidates[0]["suggested_action"], json!("discard"));
    assert!(candidates[0].get("preview").is_none(), "{candidates:?}");
}

#[test]
fn parse_candidates_rejects_everything_else_with_usage_errors() {
    for (raw, fragment) in [
        ("{not json", "JSON"),
        (r#"{"items": [{"item_id": "a"}]}"#, "candidates"),
        (r#"{"candidates": "two"}"#, "array"),
        (r#"{"candidates": 3}"#, "array"),
        ("\"just a string\"", "array"),
        ("42", "array"),
        ("null", "array"),
        (r#"[{"noise_score": 0.5}]"#, "item_id"),
        (r#"[{"item_id": 7}]"#, "item_id"),
        (r#"["a"]"#, "objects"),
        (r#"[{"item_id": "a", "noise_score": "high"}]"#, "number"),
        (r#"[{"item_id": "a", "suggested_action": 5}]"#, "string"),
    ] {
        let error = parse_candidates(raw).expect_err(raw);
        assert_eq!(error.exit_code(), ExitCode::from(2), "{raw}");
        assert!(
            error.message().contains(fragment),
            "{raw}: {}",
            error.message()
        );
    }
}

#[test]
fn hygiene_write_group_declares_its_verb_and_spec() {
    assert_eq!(HygieneWriteGroup.verbs(), &["hygiene clean"][..]);
    let spec = HygieneWriteGroup.spec("hygiene clean");
    for flag in ["file", "action", "reason", "note"] {
        assert!(spec.value_flags().contains(&flag), "{flag}");
    }
    assert!(spec.bool_flags().contains(&"confirm"));
    assert!(spec.bool_flags().contains(&"dry-run"));
    assert!(HygieneWriteGroup.mutates("hygiene clean"));
    assert!(
        HygieneWriteGroup
            .refusal("hygiene clean", &json!({}))
            .is_none()
    );
}
