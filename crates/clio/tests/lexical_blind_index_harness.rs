// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the keyed lexical index through the real `clio` binary:
//! a needle written with `remember`, rebuilt with `ops reindex --target
//! lexical`, then read back through `recall`, raw SQL, a raw scan of the store
//! file, and the members of a `backup create` archive. It also pins that a
//! `recall` against a store whose key store is gone fails loudly and creates
//! no key material.
//!
//! ## Owns
//! - The end-to-end no-plaintext evidence for the stored lexical form
//! - The archive-member scan over a backup of a keyed store
//! - The read-path regression: no key-store file is created by a search
//!
//! ## Does not own
//! - The lexical encoding under test (`clio-store`'s lexical modules)
//! - The CLI argument mapping (`cli_*` modules)
//!
//! ## Boundary
//!
//! Tests here break when the real binary stops keeping the index opaque or
//! starts creating key material on a read path; they never change either.
//!
//! Keep this module focused on the spawned-binary lexical evidence.

use clio_backup::{extract_members, read_manifest};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Needle words no schema, code path, or other test text contains.
const NEEDLE_WORDS: &[&str] = &["quadneedlewombat", "zorbhunting", "vexcalibur"];

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Unique temp directory for one scenario.
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "clio-lexical-harness-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment, the coverage pattern, and the
/// scenario's backup directory, so a snapshot never lands in a real data dir.
fn clio(backup_dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    cmd.env("CLIO_BACKUP_DIR", backup_dir);
    // A sealed write must never reach the machine's real key store: the OS
    // keychain can prompt, and a machine holding both a keychain entry and a
    // key file makes the write fail with a two-key conflict. The scratch key
    // file is the only store this child may use.
    cmd.env("CLIO_KEY_FILE", backup_dir.join("clio.key"));
    cmd.env("CLIO_KEY_FILE_ONLY", "1");
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd
}

/// Run one command against the scenario database with piped streams.
fn run(db: &Path, backup_dir: &Path, args: &[&str]) -> Output {
    clio(backup_dir)
        .args(args)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Parse one command's stdout as JSON, with the raw streams on failure.
fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\nstdout: {}\nstderr: {}",
            stdout_of(output),
            stderr_of(output)
        )
    })
}

/// The resolved key-store path for a scratch database, the way the CLI
/// resolves it (no env, so the `--db` sibling default).
fn key_store_for(db: &Path) -> PathBuf {
    clio_config::kms_path::resolve_kms_path(&|_| None, &db.display().to_string())
        .unwrap_or_else(|| panic!("no key-store path resolved for {}", db.display()))
}

/// Seed one needle memory through the real binary and return the item id.
///
/// A CLI write drains the index, so the store is already keyed when this
/// returns; the reindex call in the test then exercises the migration command
/// over rows that already carry the new form.
fn seed_needle(db: &Path, backup_dir: &Path) -> String {
    let text = format!("{} story", NEEDLE_WORDS.join(" "));
    let output = run(
        db,
        backup_dir,
        &[
            "remember",
            &text,
            "--category",
            "tool_config",
            "--output",
            "json",
        ],
    );
    assert!(
        output.status.success(),
        "seed failed: {}",
        stderr_of(&output)
    );
    let payload = json_stdout(&output);
    payload["id"]
        .as_str()
        .expect("remember reports the item id")
        .to_owned()
}

/// Every token of the raw stored column is a `k`-prefixed 32-hex digest.
fn assert_column_is_digests(content_text: &str) {
    assert!(
        !content_text.is_empty(),
        "the indexed row has a stored column"
    );
    for term in content_text.split_whitespace() {
        let hex = term.strip_prefix('k').unwrap_or_else(|| {
            panic!("stored term {term:?} is not a `k`-prefixed digest; column: {content_text:?}")
        });
        assert_eq!(hex.len(), 32, "stored term {term:?} is 32 hex chars");
        assert!(
            hex.bytes().all(|b| b.is_ascii_hexdigit()),
            "stored term {term:?} is hex"
        );
    }
    for word in NEEDLE_WORDS {
        assert!(
            !content_text.contains(word),
            "the stored column holds the needle word {word:?}"
        );
    }
}

/// Raw bytes of one file, for the needle scans.
fn file_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every extracted archive member, as bytes.
fn archive_member_bytes(archive: &Path, scratch: &Path) -> Vec<(String, Vec<u8>)> {
    let manifest = read_manifest(archive).expect("read the archive manifest");
    let extracted = extract_members(archive, &manifest, scratch).expect("extract members");
    let mut out = Vec::new();
    let payload = &extracted.payload;
    out.push((payload.member.clone(), file_bytes(payload.file.path())));
    if let Some(key) = &extracted.key {
        out.push((key.member.clone(), file_bytes(key.file.path())));
    }
    for member in &extracted.dump_tables {
        out.push((member.member.clone(), file_bytes(member.file.path())));
    }
    out
}

/// The end-to-end and archive scenarios through the real binary: the needle is absent
/// from the stored column, the store file, and every backup archive member,
/// while recall still finds the item.
#[test]
fn indexed_store_keeps_no_readable_word_in_file_column_or_archive() {
    let dir = temp_dir("no-plaintext");
    let backup_dir = dir.join("backups");
    let db = dir.join("clio.db");
    let item_id = seed_needle(&db, &backup_dir);

    // The migration command over the already-keyed rows.
    let reindex = run(
        &db,
        &backup_dir,
        &[
            "ops",
            "reindex",
            "--target",
            "lexical",
            "--dry-run",
            "false",
            "--confirm",
        ],
    );
    assert!(
        reindex.status.success(),
        "reindex failed: {}",
        stderr_of(&reindex)
    );

    // Recall still finds the item (through the binary).
    let recall = run(
        &db,
        &backup_dir,
        &["recall", NEEDLE_WORDS[0], "--output", "json"],
    );
    assert!(
        recall.status.success(),
        "recall failed: {}",
        stderr_of(&recall)
    );
    let payload = json_stdout(&recall);
    let hits = payload["hits"].as_array().expect("hits array");
    assert!(
        hits.iter()
            .any(|h| h["item_id"].as_str() == Some(item_id.as_str())),
        "recall found the needle item: {payload}"
    );

    // Raw SQL read of the stored column, not through the store API.
    let conn =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the store read-only");
    let content_text: String = conn
        .query_row("SELECT content_text FROM items_fts LIMIT 1", [], |row| {
            row.get(0)
        })
        .expect("read items_fts");
    assert_column_is_digests(&content_text);

    // Raw file scan: the needle words appear nowhere in the store file, while
    // the item id (which the store really wrote) is present.
    let bytes = file_bytes(&db);
    for word in NEEDLE_WORDS {
        let needle = word.as_bytes();
        assert_eq!(
            bytes.windows(needle.len()).filter(|w| *w == needle).count(),
            0,
            "the store file holds the needle word {word:?}"
        );
    }
    let id_marker = item_id.as_bytes();
    assert!(
        bytes.windows(id_marker.len()).any(|w| w == id_marker),
        "the scan sees bytes the store really wrote"
    );

    // The backup archive is taken and inspected member by member.
    let backup = run(&db, &backup_dir, &["backup", "create", "--output", "json"]);
    assert!(
        backup.status.success(),
        "backup create failed: {}",
        stderr_of(&backup)
    );
    let report = json_stdout(&backup);
    let archive = PathBuf::from(report["path"].as_str().expect("archive path"));
    assert!(archive.exists(), "archive at {}", archive.display());
    let staging = dir.join("extracted");
    std::fs::create_dir_all(&staging).expect("staging directory");
    let members = archive_member_bytes(&archive, &staging);
    assert!(members.len() >= 2, "payload and key members extracted");
    for (name, bytes) in &members {
        for word in NEEDLE_WORDS {
            let needle = word.as_bytes();
            assert_eq!(
                bytes.windows(needle.len()).filter(|w| *w == needle).count(),
                0,
                "archive member {name} holds the needle word {word:?}"
            );
        }
    }

    std::fs::remove_dir_all(&dir).expect("clean the scratch directory");
}

/// The read-path regression through the real binary: with the key store gone,
/// `recall` fails loudly with the key-store error and creates no key-store
/// file. Before the fix this silently created a key store holding only the
/// blind-index slot, and a following `backup create` reported key material it
/// cannot use.
#[test]
fn recall_against_a_missing_key_store_fails_loudly_and_creates_nothing() {
    let dir = temp_dir("recall-no-kms");
    let backup_dir = dir.join("backups");
    let db = dir.join("clio.db");
    seed_needle(&db, &backup_dir);
    let kms_path = key_store_for(&db);
    assert!(kms_path.exists(), "the write path created the key store");
    std::fs::remove_file(&kms_path).expect("remove the key store");

    let recall = run(
        &db,
        &backup_dir,
        &["recall", NEEDLE_WORDS[0], "--output", "json"],
    );
    assert!(
        !recall.status.success(),
        "a recall without the blind-index key must fail, got: {}",
        stdout_of(&recall)
    );
    let stderr = stderr_of(&recall);
    assert!(
        stderr.contains("blind-index key"),
        "the error names the missing blind-index key: {stderr}"
    );
    assert!(
        !kms_path.exists(),
        "a failed recall must not recreate the key store"
    );

    std::fs::remove_dir_all(&dir).expect("clean the scratch directory");
}
