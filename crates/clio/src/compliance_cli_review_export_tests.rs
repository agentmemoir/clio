// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio compliance review-export` binding: the command
//! writes the ordered content-free review file through the in-process
//! `review_export` tool, honors `--limit` and `--cursor`, names the
//! destination in the text-mode success line, refuses an existing destination,
//! refuses an unprivileged actor, and refuses a missing `--out`.
//!
//! ## Owns
//! - The injected runner and the on-disk event seed every review-export test
//!   drives
//!
//! ## Does not own
//! - The command implementation under test (`compliance_cli`, `cli_write_portability`)
//! - The writer's custody and event semantics (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use serde_json::{Value, json};

/// Run one `compliance` invocation against injected streams and a scratch cwd.
fn run_compliance(args: &[&str], current_dir: &Path) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::Cursor::new(Vec::new());
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        super::run(
            &mut io,
            crate::cli_read_test_support::tokens(args),
            Ok(current_dir.to_path_buf()),
        )
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// A unique scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clio-compliance-review-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Seed one completed local event into the on-disk store so the review file has
/// an ordered, content-free row to export.
fn seed_local_event(store: &Path, action_id: &str) {
    let options = clio_mcp::runtime::McpOpenOptions {
        backend: "sqlite".to_owned(),
        database_url: store.display().to_string(),
        ..clio_mcp::runtime::McpOpenOptions::default()
    };
    let state = clio_mcp::runtime::McpState::open_with_effective(&options, &json!({}))
        .expect("open the store");
    state
        .events
        .record_intent(&clio_store::LocalEventIntent {
            action_id,
            actor: "harness",
            scope: "review",
            kind: "review_event",
            event_time: "2026-01-01T00:00:00Z",
        })
        .expect("record intent");
    state
        .events
        .record_outcome(action_id, false, "SECRET-MARKER")
        .expect("record outcome");
}

/// The command writes the ordered content-free file under destination custody.
#[test]
fn review_export_writes_a_content_free_file() {
    let scratch = Scratch::new("writes");
    let store = scratch.path().join("store.db");
    seed_local_event(&store, "rev-1");
    let dest = scratch.path().join("review.txt");
    let dest_arg = dest.display().to_string();
    let store_arg = store.display().to_string();
    let (code, out, err) = run_compliance(
        &[
            "review-export",
            "--out",
            &dest_arg,
            "--actor",
            "harness",
            "--db",
            &store_arg,
            "-o",
            "json",
        ],
        scratch.path(),
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload: Value = serde_json::from_str(out.trim()).expect("json payload");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["destination"], json!(dest_arg), "{payload}");
    let content = std::fs::read_to_string(&dest).expect("review file exists");
    assert!(content.contains("rev-1\tallowed"), "{content}");
    assert!(
        !content.contains("SECRET-MARKER"),
        "the outcome text must not reach the file: {content}"
    );
}

/// An existing caller file is never overwritten.
#[test]
fn review_export_refuses_an_existing_destination() {
    let scratch = Scratch::new("exists");
    let store = scratch.path().join("store.db");
    let dest = scratch.path().join("review.txt");
    std::fs::write(&dest, "KEEP").expect("write the existing destination");
    let dest_arg = dest.display().to_string();
    let store_arg = store.display().to_string();
    let (code, out, err) = run_compliance(
        &[
            "review-export",
            "--out",
            &dest_arg,
            "--actor",
            "harness",
            "--db",
            &store_arg,
            "-o",
            "json",
        ],
        scratch.path(),
    );
    assert_eq!(code, ExitCode::from(crate::cli_error::EXIT_OPERATIONAL));
    let payload: Value = serde_json::from_str(err.trim()).expect("structured error envelope");
    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert_eq!(payload["code"], json!("invalid_argument"), "{payload}");
    assert!(out.trim().is_empty(), "nothing on stdout: {out}");
    assert_eq!(
        std::fs::read_to_string(&dest).expect("destination still readable"),
        "KEEP",
        "the existing file must be untouched"
    );
}

/// An unprivileged (default agent) actor is refused before any file is written.
#[test]
fn review_export_refuses_an_unprivileged_actor() {
    let scratch = Scratch::new("forbidden");
    let store = scratch.path().join("store.db");
    let dest = scratch.path().join("review.txt");
    let dest_arg = dest.display().to_string();
    let store_arg = store.display().to_string();
    let (code, out, err) = run_compliance(
        &[
            "review-export",
            "--out",
            &dest_arg,
            "--db",
            &store_arg,
            "-o",
            "json",
        ],
        scratch.path(),
    );
    assert_eq!(code, ExitCode::from(crate::cli_error::EXIT_OPERATIONAL));
    let payload: Value = serde_json::from_str(err.trim()).expect("structured error envelope");
    assert_eq!(payload["ok"], json!(false), "{payload}");
    assert_eq!(
        payload["code"],
        json!("forbidden"),
        "the actor gate must refuse with the forbidden code: {payload}"
    );
    assert!(!dest.exists(), "no file may be written: {out}");
}

/// `--limit` and `--cursor` bound the ordered rows that reach the file.
///
/// The cursor run comes first so the cursor export's own delivery event, which
/// is recorded after its rows are read, is behind the cursor in the later run.
#[test]
fn review_export_honors_limit_and_cursor() {
    let scratch = Scratch::new("page");
    let store = scratch.path().join("store.db");
    seed_local_event(&store, "rev-1");
    seed_local_event(&store, "rev-2");
    let store_arg = store.display().to_string();
    let cursor_dest = scratch.path().join("cursor.txt");
    let cursor_dest_arg = cursor_dest.display().to_string();
    let (code, out, err) = run_compliance(
        &[
            "review-export",
            "--out",
            &cursor_dest_arg,
            "--cursor",
            "1",
            "--actor",
            "harness",
            "--db",
            &store_arg,
            "-o",
            "json",
        ],
        scratch.path(),
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(
        std::fs::read_to_string(&cursor_dest).expect("cursor page exists"),
        "2\trev-2\tallowed\n",
        "the cursor must skip rows at or before the token"
    );
    let limit_dest = scratch.path().join("limit.txt");
    let limit_dest_arg = limit_dest.display().to_string();
    let (code, out, err) = run_compliance(
        &[
            "review-export",
            "--out",
            &limit_dest_arg,
            "--limit",
            "1",
            "--actor",
            "harness",
            "--db",
            &store_arg,
            "-o",
            "json",
        ],
        scratch.path(),
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(
        std::fs::read_to_string(&limit_dest).expect("limited page exists"),
        "1\trev-1\tallowed\n",
        "the limit must truncate the ordered rows"
    );
}

/// The text-mode success line names the destination.
#[test]
fn review_export_text_mode_names_the_destination() {
    let scratch = Scratch::new("text");
    let store = scratch.path().join("store.db");
    seed_local_event(&store, "rev-1");
    let dest = scratch.path().join("review.txt");
    let dest_arg = dest.display().to_string();
    let store_arg = store.display().to_string();
    let (code, out, err) = run_compliance(
        &[
            "review-export",
            "--out",
            &dest_arg,
            "--actor",
            "harness",
            "--db",
            &store_arg,
            "-o",
            "text",
        ],
        scratch.path(),
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(out, format!("review export wrote {dest_arg}\n"), "{out}");
    assert!(err.is_empty(), "{err}");
}

/// A missing `--out` is a usage error (exit 2).
#[test]
fn review_export_requires_out() {
    let scratch = Scratch::new("usage");
    let store = scratch.path().join("store.db");
    let store_arg = store.display().to_string();
    let (code, out, err) = run_compliance(
        &["review-export", "--actor", "harness", "--db", &store_arg],
        scratch.path(),
    );
    assert_eq!(
        code,
        ExitCode::from(crate::cli_error::EXIT_USAGE),
        "{out}{err}"
    );
    assert!(err.contains("--out"), "{err}");
}
