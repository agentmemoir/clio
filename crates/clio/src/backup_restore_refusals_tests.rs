// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the restore's archive-content refusals: the backend
//! mismatch, and the missing keys a restore refuses unless the operator
//! accepted the loss.
//!
//! ## Owns
//! - The acceptance, the refusal, and the hint of each refusal, asserted at
//!   the refusal functions' own boundary
//!
//! ## Does not own
//! - The plan-level refusal runs (see `backup_restore_flow_plan_tests`)
//! - The exit-code table (see `backup_restore_exit_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a refusal stops refusing, accepts what it refused,
//! or loses the hint that tells the operator what to do.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::cli_args::{self, FlagSpec};
use std::path::PathBuf;
use std::process::ExitCode;

/// A manifest of `backend` whose key store is present when `keys_included`.
fn manifest(backend: &str, keys_included: bool) -> BackupManifest {
    clio_backup::BackupManifest {
        kind: clio_backup::KIND.to_owned(),
        format_version: clio_backup::FORMAT_VERSION,
        created_at: "2026-10-30T00:00:00Z".to_owned(),
        tool_version: "clio 0.1.0".to_owned(),
        store: clio_backup::StoreInfo {
            backend: backend.to_owned(),
            slug: "clio".to_owned(),
            database: "clio".to_owned(),
            schema: None,
        },
        snapshot: clio_backup::SnapshotInfo {
            method: "quorum-read".to_owned(),
            consistency: "eventual".to_owned(),
            started_at: "2026-10-30T00:00:00Z".to_owned(),
            duration_ms: 1,
        },
        payload: clio_backup::PayloadInfo {
            member: "store.db".to_owned(),
            bytes: 8,
            sha256: "0".repeat(64),
            uncompressed_bytes: 8,
        },
        counts: clio_backup::CountsInfo { complete: true },
        schema_version: None,
        keys: clio_backup::KeysInfo {
            included: keys_included,
            member: None,
            subjects: 0,
            bytes: None,
            note: String::new(),
        },
        label: None,
        note: None,
        compression: clio_backup::COMPRESSION.to_owned(),
        authority: None,
    }
}

/// A SQLite target of `backend`, the shape the refusals compare against.
fn target(backend: &str) -> ResolvedTarget {
    ResolvedTarget {
        url: "clio.db".to_owned(),
        backend: backend.to_owned(),
        database: "clio".to_owned(),
        display: "clio.db".to_owned(),
        path: PathBuf::from("clio.db"),
        kms_path: None,
    }
}

/// Parsed restore flags over `args`.
fn parsed(args: &[&str]) -> ParsedArgs {
    let spec = FlagSpec::new(
        &[],
        &[
            "latest",
            "yes",
            "no-pre-backup",
            "allow-missing-keys",
            "dry-run",
        ],
    );
    cli_args::parse(&spec, args.iter().map(|arg| (*arg).to_owned())).expect("parse flags")
}

#[test]
fn a_keyless_archive_refuses_with_the_hint_that_names_the_choice() {
    let error = refuse_missing_keys(&manifest("sqlite", false), &parsed(&[]))
        .expect_err("a keyless archive without the flag refuses");

    assert_eq!(error.code(), "missing_keys", "{error:?}");
    assert_eq!(error.exit_code(), ExitCode::from(2), "{}", error.message());
    assert!(
        error
            .hint()
            .is_some_and(|hint| hint.contains("allow-missing-keys")),
        "the hint names the flag and the alternative: {error:?}"
    );
}

#[test]
fn a_keyless_archive_with_the_flag_is_accepted() {
    refuse_missing_keys(
        &manifest("sqlite", false),
        &parsed(&["--allow-missing-keys"]),
    )
    .expect("the operator accepted the loss");
}

#[test]
fn a_keyed_archive_is_accepted() {
    refuse_missing_keys(&manifest("sqlite", true), &parsed(&[]))
        .expect("a keyed archive refuses nothing");
}

#[test]
fn an_archive_of_another_backend_refuses_naming_both() {
    let error = refuse_backend_mismatch(&manifest("postgres", true), &target("sqlite"))
        .expect_err("a backend mismatch refuses");

    assert_eq!(error.code(), "backend_mismatch", "{error:?}");
    assert_eq!(error.exit_code(), ExitCode::from(2), "{}", error.message());
    assert!(
        error.message().contains("postgres") && error.message().contains("sqlite"),
        "the refusal names both backends: {error:?}"
    );
    assert!(
        error.hint().is_some_and(|hint| hint.contains("sqlite")),
        "the hint names the backend to restore from: {error:?}"
    );
}

#[test]
fn an_archive_of_the_target_backend_is_accepted() {
    refuse_backend_mismatch(&manifest("sqlite", true), &target("sqlite"))
        .expect("a matching backend refuses nothing");
}
