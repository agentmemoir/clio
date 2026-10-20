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
//! - `ok`, `assert_forbidden`, `assert_scope_change_refusal`,
//!   `assert_damaged_journal_refusal`, `assert_open_copy`, and
//!   `assert_unknown_product_path_refusal`
//! - `seed_done_path`, the shared done product path a refusal is measured
//!   against
//!
//! ## Does not own
//! - The CLI driver and the scratch, journal, and backup paths (see `cli_test_support` and
//!   `case_fixtures`)
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

use crate::case_fixtures::{backup_dir, journal_file, scratch};
use crate::cli_test_support::{self, Refusal, assert_refusal};
use clio::ErrorCode;
use clio::store::DurableRequestStore;
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{PathOwner, PathSlot, PathState};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;

/// The exact refusal the production returns when a required action of the
/// request is still open: the closure refusal the request service builds.
///
/// The error code is the public enum's own string, so a rename of the code
/// fails this check here instead of leaving a copied string behind.
const OPEN_COPY_REFUSAL: Refusal = Refusal {
    exit: 1,
    code: ErrorCode::Forbidden.as_str(),
    message: "request closure refused: OpenCopyAction",
};

/// The exact refusal the production returns when a restore reads a damaged
/// live erasure journal: the authority refusal the restore flow builds.
const DAMAGED_JOURNAL_REFUSAL: Refusal = Refusal {
    exit: 2,
    code: ErrorCode::ConfigCorrupt.as_str(),
    message: "the erasure journal is damaged",
};

/// The exact refusal the production returns when a request id is opened again
/// under a different kind or scope: the ledger's binding refusal.
const SCOPE_CHANGE_REFUSAL: Refusal = Refusal {
    exit: 1,
    code: ErrorCode::Forbidden.as_str(),
    message: "request id is already bound to a different kind or scope",
};

/// The exact refusal the production returns when the closure names a product
/// path this build does not know: the closure refusal the request service
/// builds.
const UNKNOWN_PRODUCT_PATH_REFUSAL: Refusal = Refusal {
    exit: 1,
    code: ErrorCode::Forbidden.as_str(),
    message: "request closure refused: UnknownProductPath",
};

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

    /// Run one `clio` command to completion under a bounded wait.
    ///
    /// The wait is bounded: a command that does not exit before the driver's
    /// deadline is killed and reaped, and the run fails instead of hanging.
    fn spawn(&self, kms: &Path, args: &[&str]) -> Output {
        let mut cmd = cli_test_support::clio_command(&self.journal, kms, &self.backups);
        cmd.env("CLIO_DEPLOYMENT_CONFIG", &self.deployment);
        if let Some(key) = &self.key {
            cmd.env("CLIO_KEY_FILE", key).env("CLIO_KEY_FILE_ONLY", "1");
        }
        cmd.args(args);
        cli_test_support::run_captured(&mut cmd)
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

    /// Run one `backup restore` into `target`, the store the restore would
    /// place the archive into.
    ///
    /// The target is named by the caller, so the store a refusal is measured
    /// against is the store the restore would have written to.
    pub(crate) fn restore(&self, kms: &Path, id: &str, target: &Path) -> Output {
        let line = format!(
            "backup restore {id} --yes --db {} -o json",
            target.to_string_lossy()
        );
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
    envelope_text(&String::from_utf8_lossy(&out.stderr))
}
/// The refusal envelope the CLI wrote to its stderr.
fn envelope_text(stderr: &str) -> Value {
    serde_json::from_str(stderr).unwrap_or_else(|error| panic!("refusal not JSON: {error}"))
}
pub(crate) fn assert_forbidden(out: &Output) {
    let (_, stderr) = streams(out);
    assert_eq!(out.status.code(), Some(1), "must refuse (exit 1): {stderr}");
    assert_eq!(
        envelope(out)["code"],
        json!(ErrorCode::Forbidden.as_str()),
        "must be forbidden"
    );
}
/// The changed-scope refusal, pinned to the production's exact message, error
/// code, and exit code.
///
/// The message, the code, and the exit are the production's own, so a refusal
/// that changes in any of them fails this check instead of still matching on a
/// fragment of the old text.
pub(crate) fn assert_scope_change_refusal(out: &Output) {
    let (_, stderr) = streams(out);
    assert_refusal(out.status.code(), &stderr, &SCOPE_CHANGE_REFUSAL);
}
/// The damaged journal's own refusal, pinned to the production's exact
/// message, error code, and exit code.
///
/// The message, the code, and the exit are the production's own, so a refusal
/// that changes in any of them fails this check instead of still matching on a
/// fragment of the old text.
pub(crate) fn assert_damaged_journal_refusal(out: &Output) {
    let (_, stderr) = streams(out);
    assert_refusal(out.status.code(), &stderr, &DAMAGED_JOURNAL_REFUSAL);
}
/// The open copy action's own refusal, pinned to the production's exact
/// message, error code, and exit code.
///
/// The message, the code, and the exit are the production's own, so a refusal
/// that changes in any of them fails this check instead of still matching on a
/// fragment of the old text.
pub(crate) fn assert_open_copy(out: &Output) {
    let (_, stderr) = streams(out);
    assert_open_copy_refusal(out.status.code(), &stderr);
}
/// The same open-copy refusal for a run whose exit code and stderr are
/// already captured.
pub(crate) fn assert_open_copy_refusal(code: Option<i32>, stderr: &str) {
    assert_refusal(code, stderr, &OPEN_COPY_REFUSAL);
}
/// The unknown product path's own refusal, pinned to the production's exact
/// message, error code, and exit code.
///
/// The message, the code, and the exit are the production's own, so a refusal
/// that changes in any of them fails this check instead of still matching on a
/// fragment of the old text.
pub(crate) fn assert_unknown_product_path_refusal(out: &Output) {
    let (_, stderr) = streams(out);
    assert_refusal(out.status.code(), &stderr, &UNKNOWN_PRODUCT_PATH_REFUSAL);
}
/// Mark one named product path done on a stored request.
///
/// This is the shared helper every scenario uses: a scenario that needs a done
/// path calls it here, so the target carries no second copy of this behaviour.
/// The path is named by the caller, and every failure names the request and
/// the path it is about.
pub(crate) fn seed_done_path(store: &dyn DurableRequestStore, request_id: &str, path: &str) {
    let row = store
        .load(request_id)
        .unwrap_or_else(|error| panic!("load the request {request_id}: {error}"))
        .unwrap_or_else(|| panic!("the request {request_id} has no stored row"));
    let mut record = row_to_record(&row)
        .unwrap_or_else(|error| panic!("decode the stored request {request_id}: {error}"));
    let done = PathSlot {
        owner: PathOwner::Clio,
        state: PathState::Done,
    };
    record.paths.insert(path.to_owned(), done);
    store
        .save(&record_to_row(&record))
        .unwrap_or_else(|error| panic!("save the request {request_id}: {error}"));
}
