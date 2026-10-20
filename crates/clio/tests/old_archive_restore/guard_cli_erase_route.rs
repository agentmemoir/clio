// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving a composed SQLite compliance flow
//! through real `clio` commands: seed the denied and the control subject, open
//! the deployment journal, cut an archive, erase the denied subject through the
//! public route, then prove every real `clio backup restore` route refuses that
//! pre-erasure archive, and refuses it because it carries the erased subject's
//! keys, while the unrelated subject stays readable.
//!
//! ## Owns
//! - The SQLite composed remember/create/erase/restore refusal and its evidence
//!   line
//! - The attribution of that refusal to the erased subject
//! - The per-route refusal check and the target's readable control subject
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
    CliOutput, DENIED_SUBJECT_REFUSAL, assert_refused_exactly, backup_dir, file_sha256,
    journal_file, run_cli, run_restore, subject_hash,
};
use crate::support::{BANK, Scratch};
use clio_compliance::{JournalState, initialize_new_empty, load_journal};
use serde_json::Value;
use std::path::Path;

/// The marker text of the subject the composed flow erases.
const DENIED_TEXT: &str = "denied sentinel note";

/// The marker text of the subject that must stay readable in the target.
const CONTROL_TEXT: &str = "control sentinel note";

/// Assert one real `clio` command succeeded, naming its stderr on failure.
fn run_ok(journal: &Path, kms: &Path, backups: &Path, args: &[&str]) -> CliOutput {
    let out = run_cli(journal, kms, backups, args);
    assert!(
        out.code == Some(0),
        "`clio {}` failed: {}",
        args.join(" "),
        out.stderr
    );
    out
}

/// The last non-empty stdout line parsed as JSON: the command's report.
fn last_json(stdout: &str) -> Value {
    let line = stdout
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

/// The subject ids the archive's payload member records for its keys.
///
/// This is the table the restore authority check reads to find a member's
/// subject references, so what it holds is what the refusal is made of.
fn archived_subject_ids(archive: &Path, dir: &Path) -> Vec<String> {
    let manifest = clio_backup::read_manifest(archive).expect("read the archive manifest");
    std::fs::create_dir_all(dir).expect("create the extraction directory");
    let extracted =
        clio_backup::extract_members(archive, &manifest, dir).expect("extract the archive members");
    let connection = rusqlite::Connection::open(extracted.payload.file.path())
        .expect("open the archived payload");
    connection
        .execute_batch("PRAGMA query_only = ON")
        .expect("keep the archived payload read-only");
    let mut statement = connection
        .prepare("SELECT subject_id FROM subject_keys")
        .expect("prepare the subject keys read");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("read the archived subject keys")
        .map(|row| row.expect("a subject key row"))
        .collect()
}

/// Assert the target's own subject is still readable after one refused route.
///
/// The target keeps its own subject, so a placement changes it and this check
/// catches that.
fn assert_control_readable(
    journal: &Path,
    target_kms: &Path,
    backups: &Path,
    target: &str,
    control: &str,
    selector: &[&str],
) {
    let readback = run_cli(
        journal,
        target_kms,
        backups,
        &[
            "--db", target, "get", control, "--bank", BANK, "--output", "json",
        ],
    );
    assert!(
        readback.code == Some(0),
        "the target subject must stay readable after {selector:?}: {}",
        readback.stderr
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

    // The deployment journal starts empty, so the archive cut next records a
    // live authority and the only denial the journal holds afterwards is the
    // erased subject's.
    initialize_new_empty(&journal).expect("initialize the deployment journal");

    let denied = remember(&journal, &source_kms, &backups, source, BANK, DENIED_TEXT);
    remember(&journal, &source_kms, &backups, source, BANK, CONTROL_TEXT);

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

    // The archive was cut before the erasure, so it still carries the erased
    // subject's key: that is what the restore authority check reads, and what
    // makes the refusal below attributable to the erased subject.
    let subject_ids = archived_subject_ids(Path::new(&archive_path), &scratch.join("classified"));
    assert!(
        subject_ids.contains(&denied),
        "the archive must record the erased subject's key: {subject_ids:?}"
    );

    // The live journal denies the erased subject alone, so no other denial can
    // explain the refusal the routes produce.
    let JournalState::Current(live) = load_journal(&journal) else {
        panic!("the live journal must be current");
    };
    assert_eq!(
        live.denials.len(),
        1,
        "the journal must deny the erased subject alone: {live:?}"
    );
    assert_eq!(
        live.denials[0].subject_hash,
        subject_hash(&denied),
        "the journal must deny the erased subject"
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
        assert_refused_exactly(&refused, &DENIED_SUBJECT_REFUSAL);
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
        assert_control_readable(
            &journal,
            &target_kms,
            &backups,
            target,
            &target_control,
            &selector,
        );
    }

    println!("REFUSED sqlite composed-erase routes=4 archive={snapshot}");
}
