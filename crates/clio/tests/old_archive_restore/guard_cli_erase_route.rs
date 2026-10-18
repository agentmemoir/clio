// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving a composed SQLite compliance flow
//! through real `clio` commands: seed two subjects, open the deployment journal,
//! cut an archive, erase one subject through the public route, then prove every
//! real `clio backup restore` route refuses that pre-erasure archive while the
//! unrelated subject stays readable.
//!
//! ## Owns
//! - The SQLite composed remember/create/erase/restore refusal and its evidence
//!   line
//! - The small CLI invocation and JSON-line helpers the composed flow uses
//!
//! ## Does not own
//! - The restore refusals after a synthetic denial (see `guard_cli`)
//! - The erasure-related SQLite refusals (see `guard_cli_erasure`)
//! - The real `clio backup restore` driver and shared fixtures (see
//!   `guard_fixtures`)
//! - The Postgres half (see `guard_cli_pg`)
//!
//! ## Boundary
//!
//! Changes to which composed erase/restore refusals are measured should
//! normally require changes here. Changes to the guard or the CLI should not.
//!
//! Keep this module focused on the composed real-command SQLite refusal.

use crate::guard_fixtures::{
    assert_refused, backup_dir, clio_command, file_sha256, journal_file, run_restore,
};
use crate::support::{BANK, Scratch};
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;

/// The marker text of the throwaway subject whose erasure opens the deployment
/// journal, so the archive cut next records a live erasure authority.
const SEED_TEXT: &str = "seed sentinel note";

/// The marker text of the subject the composed flow erases.
const DENIED_TEXT: &str = "denied sentinel note";

/// The marker text of the subject that must stay readable in the target.
const CONTROL_TEXT: &str = "control sentinel note";

/// Run one real `clio` command to completion, draining both streams.
fn run(journal: &Path, kms: &Path, backups: &Path, args: &[&str]) -> std::process::Output {
    clio_command(journal, kms, backups)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|err| panic!("run clio {}: {err}", args.join(" ")))
}

/// Assert one real `clio` command succeeded, naming its stderr on failure.
fn run_ok(journal: &Path, kms: &Path, backups: &Path, args: &[&str]) -> std::process::Output {
    let out = run(journal, kms, backups, args);
    assert!(
        out.status.success(),
        "`clio {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// The last non-empty stdout line parsed as JSON: the command's report.
fn last_json(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    serde_json::from_str(line)
        .unwrap_or_else(|err| panic!("parse the last stdout line {line:?}: {err}"))
}

/// Remember one marker in `bank` under `db` and return the item id, which is
/// also the subject id the public erase route names.
fn remember(
    journal: &Path,
    kms: &Path,
    backups: &Path,
    db: &str,
    bank: &str,
    text: &str,
) -> String {
    let out = run_ok(
        journal,
        kms,
        backups,
        &[
            "--db",
            db,
            "remember",
            text,
            "--category",
            "schema",
            "--actor",
            "agent",
            "--bank",
            bank,
            "--output",
            "json",
        ],
    );
    last_json(&out.stdout)["id"]
        .as_str()
        .expect("the remembered item id")
        .to_owned()
}

/// Erase one subject through the real public `clio erase` route.
fn erase(journal: &Path, kms: &Path, backups: &Path, db: &str, subject: &str, request: &str) {
    run_ok(
        journal,
        kms,
        backups,
        &[
            "--db",
            db,
            "erase",
            "--subject",
            subject,
            "--basis",
            "gdpr-art-17",
            "--request",
            request,
            "--confirm",
            "--actor",
            "harness",
            "--bank",
            BANK,
            "--output",
            "json",
        ],
    );
}

#[test]
fn sqlite_real_erase_refuses_every_pre_erasure_restore_route() {
    let scratch = Scratch::new("guard-cli-erase-route");
    let backups = backup_dir(&scratch);
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    let source_kms = scratch.join("source/clio.kms.json");
    let source_db = scratch.join("source/clio.db");
    let source = source_db.to_str().expect("the source path is UTF-8");

    let seed = remember(&journal, &source_kms, &backups, source, BANK, SEED_TEXT);
    let denied = remember(&journal, &source_kms, &backups, source, BANK, DENIED_TEXT);
    remember(&journal, &source_kms, &backups, source, BANK, CONTROL_TEXT);

    // The throwaway erasure opens the deployment journal, so the archive cut
    // next records a live authority every restore route can classify.
    erase(&journal, &source_kms, &backups, source, &seed, "req-seed");

    let created = last_json(
        &run_ok(
            &journal,
            &source_kms,
            &backups,
            &["--db", source, "backup", "create", "--output", "json"],
        )
        .stdout,
    );
    assert!(
        created["authority_warning"].is_null(),
        "the archive must record a live erasure authority: {created}"
    );
    let snapshot = created["snapshot_id"]
        .as_str()
        .expect("the snapshot id")
        .to_owned();
    let archive_path = created["path"]
        .as_str()
        .expect("the archive path")
        .to_owned();

    // The real public erasure route records the denial of the target subject.
    erase(
        &journal,
        &source_kms,
        &backups,
        source,
        &denied,
        "req-denied",
    );

    // The target keeps its own readable subject, so a placement changes it and
    // the read-back below catches that.
    let target_kms = scratch.join("target/clio.kms.json");
    let target_db = scratch.join("target/clio.db");
    let target = target_db.to_str().expect("the target path is UTF-8");
    let target_control = remember(&journal, &target_kms, &backups, target, BANK, CONTROL_TEXT);

    let archive_before = file_sha256(Path::new(&archive_path));
    let journal_before = file_sha256(&journal);

    for selector in [
        vec![snapshot.as_str()],
        vec![snapshot.as_str(), "--no-pre-backup"],
        vec![snapshot.as_str(), "--dry-run"],
        vec!["--latest"],
    ] {
        let refused = run_restore(&journal, &target_kms, &backups, target, &selector);
        assert_refused(&refused, "denied subject");
        assert_eq!(
            file_sha256(&journal),
            journal_before,
            "selector {selector:?} changed the journal authority"
        );
        assert_eq!(
            file_sha256(Path::new(&archive_path)),
            archive_before,
            "selector {selector:?} sanitized the archive"
        );
        let readback = run(
            &journal,
            &target_kms,
            &backups,
            &[
                "--db",
                target,
                "get",
                &target_control,
                "--bank",
                BANK,
                "--output",
                "json",
            ],
        );
        assert!(
            readback.status.success(),
            "the target subject must stay readable after {selector:?}: {}",
            String::from_utf8_lossy(&readback.stderr)
        );
    }

    println!("REFUSED sqlite composed-erase routes=4 archive={snapshot}");
}
