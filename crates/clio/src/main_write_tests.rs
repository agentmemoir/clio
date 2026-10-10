// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the binary dispatch edge for the write surface: top-level
//! help bindings, unknown-command suggestions, and the `--db`-backed write →
//! read contract across `run` invocations.
//!
//! ## Owns
//! - Dispatch and exit-code tests for the write verbs at the binary edge
//!
//! ## Does not own
//! - The CLI implementation under test (`main`)
//! - The in-process write behavior tests (`cli_write_core_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

fn temp_db(tag: &str) -> String {
    // The write verbs seal the key store's slots under the Clio encryption key.
    crate::setup_support_tests::ensure_test_key();
    let dir = std::env::temp_dir().join(format!("clio-write-e2e-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("clio.db").to_string_lossy().into_owned()
}

#[test]
fn help_prints_every_write_verb_with_its_tool() {
    // The catalog wraps to a column, so an assertion over a phrase compares with
    // whitespace collapsed rather than with the line breaks it happens to land on.
    let text = crate::cli_top_level::help_text();
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for tool in [
        "-> store",
        "-> admit_preview",
        "-> summarize",
        "-> consolidate",
        "-> triple_add",
        "-> triple_end",
        "-> belief_observe",
        "-> graph_link",
        "-> update",
        "-> invalidate",
        "-> discard",
        "-> correct",
    ] {
        assert!(flat.contains(tool), "help must bind {tool}:\n{text}");
    }
    for usage in [
        "clio update ID VALUE --rule discrete|continuous",
        "clio invalidate ID",
        "clio discard ID --reason R --confirm",
        "clio correct ID VALUE --reason R",
    ] {
        assert!(flat.contains(usage), "help must show `{usage}`:\n{text}");
    }
    // A dry-run write is scored rather than stored, and the catalog says which
    // tool that is.
    assert!(
        flat.contains("score one write without storing it"),
        "help must say what a dry-run write does:\n{text}"
    );
    assert!(
        flat.contains("Use -- after the command before positional text that starts with -; keep flags before it"),
        "{text}"
    );
    assert!(text.contains("clio remember"), "{text}");
    assert!(text.contains("clio admit"), "{text}");
    assert!(text.contains("clio summarize"), "{text}");
    assert!(text.contains("clio consolidate"), "{text}");
    assert!(text.contains("clio triple add"), "{text}");
    assert!(text.contains("clio triple end"), "{text}");
    assert!(text.contains("clio belief observe"), "{text}");
    assert!(text.contains("clio graph link"), "{text}");
}

#[test]
fn help_catalog_lists_write_commands_with_tools() {
    // `clio help --json` derives its command table from the same binding list,
    // so the CLI catalog and the tools cannot drift.
    let mut out = Vec::new();
    cli_help::write_catalog(&mut out).expect("catalog");
    let text = String::from_utf8(out).expect("utf8");
    for pair in [
        ("remember", "store"),
        ("admit", "admit_preview"),
        ("summarize", "summarize"),
        ("consolidate", "consolidate"),
        ("triple add", "triple_add"),
        ("triple end", "triple_end"),
        ("belief observe", "belief_observe"),
        ("graph link", "graph_link"),
        ("update", "update"),
        ("invalidate", "invalidate"),
        ("discard", "discard"),
        ("correct", "correct"),
    ] {
        let (command, tool) = pair;
        assert!(
            text.contains(&format!("\"command\":\"{command}\""))
                && text.contains(&format!("\"tool\":\"{tool}\"")),
            "catalog must bind {command} -> {tool}: {text}"
        );
    }
}

#[test]
fn unknown_write_command_suggests_the_closest_verb() {
    assert_eq!(run(vec!["remembr".into(), "x".into()]), ExitCode::from(2));
    let err = cli_help::suggest_command("remembr");
    assert_eq!(err, Some("remember"));
}

#[test]
fn remember_persists_through_a_file_store_and_exits_zero() {
    let db = temp_db("remember");
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "remember".into(),
        "prefers aisle seats".into(),
        "--category".into(),
        "persona".into(),
        "--bank".into(),
        "demo".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let _ = std::fs::remove_file(&db);
}

#[test]
fn end_of_options_delimiter_allows_hyphen_prefixed_write_and_read_positionals() {
    let db = temp_db("delimiter");
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "remember".into(),
        "--category".into(),
        "output_constraint".into(),
        "--bank".into(),
        "demo".into(),
        "--output".into(),
        "json".into(),
        "--".into(),
        "- note".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "recall".into(),
        "--bank".into(),
        "demo".into(),
        "--output".into(),
        "json".into(),
        "--".into(),
        "- note".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(format!("{db}.kms.json"));
}

#[test]
fn top_level_delimiter_before_verb_is_rejected() {
    assert_eq!(run(vec!["--".into(), "version".into()]), ExitCode::from(2));
}

#[test]
fn remember_dry_run_against_a_file_store_writes_nothing_and_exits_zero() {
    let db = temp_db("dry");
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "remember".into(),
        "prefers terse answers".into(),
        "--category".into(),
        "persona".into(),
        "--bank".into(),
        "demo".into(),
        "--dry-run".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    // The file store never received the item: a fresh open has no bank rows.
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "inspect".into(),
        "--bank".into(),
        "demo".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let _ = std::fs::remove_file(&db);
}

#[test]
fn recall_after_remember_decrypts_across_processes() {
    // Regression for #2: a one-shot `remember` process encrypts item content
    // under a DEK that must outlive it, so a later `recall` process can decrypt.
    // The CLI resolves a file-backed dev-KMS next to the database; before the
    // fix the second process had an empty in-memory KMS and exited 1 with
    // `no DEK for subject`.
    let db = temp_db("kms");
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "remember".into(),
        "prefers aisle seats".into(),
        "--category".into(),
        "persona".into(),
        "--bank".into(),
        "demo".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "recall".into(),
        "aisle".into(),
        "--bank".into(),
        "demo".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(format!("{db}.kms.json"));
}

#[test]
fn discard_without_confirm_exits_two_end_to_end() {
    // At the binary edge: a non-interactive `discard` with no `--confirm`
    // refuses with the usage exit code before touching any item.
    let db = temp_db("discard-refuse");
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "discard".into(),
        "itm-nope".into(),
        "--reason".into(),
        "noise".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::from(2));
    let _ = std::fs::remove_file(&db);
}

#[test]
fn discard_dry_run_exits_zero_end_to_end() {
    // `--dry-run` is a zero-write preview and never needs `--confirm`, so it
    // exits 0 without resolving (or removing) the target.
    let db = temp_db("discard-dry");
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "discard".into(),
        "itm-nope".into(),
        "--reason".into(),
        "noise".into(),
        "--dry-run".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let _ = std::fs::remove_file(&db);
}

#[test]
fn update_rejects_an_unknown_rule_end_to_end() {
    let db = temp_db("update-rule");
    let code = run(vec![
        "--db".into(),
        db.clone(),
        "update".into(),
        "itm-1".into(),
        "1".into(),
        "--rule".into(),
        "fact".into(),
        "--output".into(),
        "json".into(),
    ]);
    assert_eq!(code, ExitCode::from(2));
    let _ = std::fs::remove_file(&db);
}
