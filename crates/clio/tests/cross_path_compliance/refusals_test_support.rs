// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared harness of the refusal scenarios
//! of the cross-path compliance proof: the one deployment the real `clio`
//! binary is driven against, and the assertions every refusal must satisfy —
//! the exit code, the error code, and the exact refusal message the production
//! builds.
//!
//! ## Owns
//! - `Env`, the one deployment the refusal scenarios drive
//! - `ok`, `assert_forbidden`, `assert_refused_exit2`, and `assert_open_copy`
//! - `seed_done_path`, the done product path a refusal is measured against
//!
//! ## Does not own
//! - The CLI driver and the scratch, journal, and backup paths (see `cli` and
//!   `fixtures`)
//! - The refusal scenarios and the claims they prove (see the `scenario_*`
//!   modules)
//!
//! ## Boundary
//!
//! Changes to how a refusal is read or to the deployment a scenario drives
//! should normally require changes here. Changes to a scenario's claims should
//! not.
//!
//! Keep this module focused on the shared refusal harness.

use crate::cli;
use crate::fixtures::{backup_dir, journal_file, scratch};
use clio::store::DurableRequestStore;
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{PathOwner, PathSlot, PathState};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

/// The exact refusal the production returns when a required action of the
/// request is still open: the closure refusal the request service builds.
const OPEN_COPY_REFUSAL_MESSAGE: &str = "request closure refused: OpenCopyAction";
/// The error code the production refusal carries.
const OPEN_COPY_REFUSAL_CODE: &str = "forbidden";
/// The process exit code the production refusal maps to.
const OPEN_COPY_REFUSAL_EXIT: i32 = 1;

/// One deployment: a scratch directory, the journal and backup paths the
/// binary reads, the key store, and the store the commands address.
pub(crate) struct Env {
    pub(crate) scratch: crate::common::Scratch,
    pub(crate) deployment: PathBuf,
    pub(crate) kms: PathBuf,
    pub(crate) journal: PathBuf,
    pub(crate) backups: PathBuf,
    db: String,
    pub(crate) key: Option<PathBuf>,
}
impl Env {
    pub(crate) fn new(tag: &str) -> Self {
        let scratch = scratch(tag);
        Self {
            deployment: scratch.join("no-such-deployment.json"),
            kms: scratch.join("clio.kms.json"),
            journal: journal_file(&scratch, "journal/erasure-journal.json"),
            backups: backup_dir(&scratch),
            db: scratch.join("clio.db").to_string_lossy().into_owned(),
            key: None,
            scratch,
        }
    }

    pub(crate) fn db(&self) -> &str {
        &self.db
    }

    /// One deployment against a Postgres schema: the same environment, pointed
    /// at the schema's URL instead of a SQLite file.
    pub(crate) fn postgres(tag: &str, url: &str) -> Self {
        Self {
            db: url.to_owned(),
            ..Self::new(tag)
        }
    }

    pub(crate) fn path(&self) -> &Path {
        Path::new(&self.db)
    }

    fn spawn(&self, kms: &Path, args: &[&str]) -> Output {
        let mut cmd = cli::clio_command(&self.journal, kms, &self.backups);
        cmd.env("CLIO_DEPLOYMENT_CONFIG", &self.deployment);
        if let Some(key) = &self.key {
            cmd.env("CLIO_KEY_FILE", key).env("CLIO_KEY_FILE_ONLY", "1");
        }
        cmd.args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run clio")
    }

    pub(crate) fn command(&self, sub: &str, rest: &str) -> Output {
        let line = format!("compliance {sub} {rest} --db {} -o json", self.db());
        let args: Vec<&str> = line.split_whitespace().collect();
        self.spawn(&self.kms, &args)
    }

    pub(crate) fn value(&self, sub: &str, rest: &str) -> Value {
        stdout_of(&self.command(sub, rest))
    }

    pub(crate) fn start(&self, request: &str, subject: &str) -> Output {
        let rest = format!("--request-id {request} --kind erasure --subject-id {subject}");
        self.command("request-start", &rest)
    }

    pub(crate) fn status(&self, request: &str) -> Value {
        self.value("request-status", &format!("--request-id {request}"))
    }

    pub(crate) fn peer_status(&self, request: &str) -> Value {
        self.value("peer-status", &format!("--request-id {request}"))
    }

    pub(crate) fn inventory(&self) -> Value {
        self.value("inventory", "")
    }

    pub(crate) fn restore(&self, kms: &Path, id: &str) -> Output {
        let line = format!("backup restore {id} --yes --db {} -o json", self.db());
        let args: Vec<&str> = line.split_whitespace().collect();
        self.spawn(kms, &args)
    }
}
fn stdout_of(out: &Output) -> Value {
    let bytes = &out.stdout;
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): out={} err={}",
            String::from_utf8_lossy(bytes),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}
pub(crate) fn ok(out: &Output, what: &str) {
    let (out_text, err_text) = streams(out);
    assert!(out.status.success(), "{what} failed: {out_text}{err_text}");
}
fn streams(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
fn envelope(out: &Output) -> Value {
    serde_json::from_slice(&out.stderr).unwrap_or_else(|error| panic!("refusal not JSON: {error}"))
}
pub(crate) fn assert_forbidden(out: &Output) {
    let (_, stderr) = streams(out);
    assert_eq!(out.status.code(), Some(1), "must refuse (exit 1): {stderr}");
    assert_eq!(
        envelope(out)["code"],
        json!("forbidden"),
        "must be forbidden"
    );
}
pub(crate) fn assert_refused_exit2(out: &Output, needle: &str) {
    let (_, stderr) = streams(out);
    assert_eq!(out.status.code(), Some(2), "must refuse (exit 2): {stderr}");
    assert!(stderr.contains(needle), "the refusal must name {needle:?}");
}
/// The open copy action's own refusal, pinned to the production's exact
/// message, error code, and exit code.
///
/// The message, the code, and the exit are the production's own, so a refusal
/// that changes in any of them fails this check instead of still matching on a
/// fragment of the old text.
pub(crate) fn assert_open_copy(out: &Output) {
    let (_, stderr) = streams(out);
    assert_eq!(
        out.status.code(),
        Some(OPEN_COPY_REFUSAL_EXIT),
        "must refuse (exit {OPEN_COPY_REFUSAL_EXIT}): {stderr}"
    );
    let refused = envelope(out);
    assert_eq!(
        refused["code"],
        json!(OPEN_COPY_REFUSAL_CODE),
        "must carry the production's error code: {stderr}"
    );
    assert_eq!(
        refused["message"],
        json!(OPEN_COPY_REFUSAL_MESSAGE),
        "must carry the production's exact refusal: {stderr}"
    );
}
pub(crate) fn seed_done_path(store: &dyn DurableRequestStore, request_id: &str, path: &str) {
    let row = store.load(request_id).expect("load").expect("the row");
    let mut record = row_to_record(&row).expect("decode the request row");
    let done = PathSlot {
        owner: PathOwner::Clio,
        state: PathState::Done,
    };
    record.paths.insert(path.to_owned(), done);
    store.save(&record_to_row(&record)).expect("save the row");
}
