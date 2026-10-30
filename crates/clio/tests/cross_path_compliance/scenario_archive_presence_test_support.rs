// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared state the archive-presence claim
//! builds: the gated SQLite and Postgres deployments, the clean archives with
//! one unclassifiable member, the real restore invocation, and the raw
//! container build.
//!
//! ## Owns
//! - `run_restore`, `raw_with_unclassifiable`, and `place_raw`
//! - `gated_sqlite`, `unknown_sqlite`, `gated_pg`, and `unknown_pg`
//!
//! ## Does not own
//! - The scenario cases (see `scenario_archive_presence`)
//! - The shared fixtures and the CLI driver (see `case_fixtures` and
//!   `support_test_support`)
//!
//! ## Boundary
//!
//! Changes to how the presence scenario builds its archives should normally
//! require changes here. Changes to what the scenario claims should not.
//!
//! Keep this module focused on the shared state, with no tests.

use crate::case_fixtures::{
    CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, open_pg, open_sqlite, seed_pg,
    seed_sqlite,
};
use crate::cli_test_support::{CliOutput, run_cli};
use crate::common;
use crate::support_pg_test_support::archive_postgres_with_authority;
use crate::support_test_support::{
    Archive, archive_sqlite_with_authority, current_authority, erase_subject,
};
use clio::store::ScopedSchema;
use clio_backup::{KEY_MEMBER, MANIFEST_MEMBER, archive_file_name, extract_members};
use clio_compliance::initialize_new_empty;
use std::path::{Path, PathBuf};

/// Run the real `clio backup restore` route with `selector`, answering the
/// confirmation with `--yes`, and return its exit and stderr.
pub(super) fn run_restore(
    journal: &Path,
    kms: &Path,
    backups: &Path,
    target: &str,
    selector: &[&str],
) -> CliOutput {
    let mut args = vec!["backup", "restore"];
    args.extend_from_slice(selector);
    args.extend(["--yes", "--db", target, "--output", "json"]);
    run_cli(journal, kms, backups, &args)
}

/// Build an archive from `archive`'s own members plus one undeclared member.
///
/// The container is hand-built because a production create only writes declared
/// members; the extra member is exactly the unclassifiable case.
pub(super) fn raw_with_unclassifiable(archive: &Archive, dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("create the raw-build directory");
    let extracted =
        extract_members(&archive.path, &archive.manifest, dir).expect("extract the archive");
    let mut members: Vec<(String, Vec<u8>)> = vec![
        (MANIFEST_MEMBER.to_owned(), archive.manifest.to_json_bytes()),
        (
            extracted.payload.member.clone(),
            std::fs::read(extracted.payload.file.path()).expect("read the payload"),
        ),
    ];
    for table in &extracted.dump_tables {
        members.push((
            table.member.clone(),
            std::fs::read(table.file.path()).expect("read a table member"),
        ));
    }
    if let Some(key) = &extracted.key {
        members.push((
            KEY_MEMBER.to_owned(),
            std::fs::read(key.file.path()).expect("read the key member"),
        ));
    }
    members.push((
        "unclassifiable.member".to_owned(),
        b"no manifest entry describes this member".to_vec(),
    ));
    let refs: Vec<(&str, &[u8])> = members
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let path = dir.join("unclassifiable.cliobak.tar.gz");
    std::fs::write(
        &path,
        clio_backup::container::archive_from_raw_members_for_tests(&refs),
    )
    .expect("write the raw archive");
    path
}

/// Place a raw archive's bytes over `id`'s archive in `backups`, so the CLI
/// lists the same id and reads the hand-built container.
pub(super) fn place_raw(backups: &Path, id: &str, raw: &Path) {
    let destination = backups.join(archive_file_name(id));
    std::fs::copy(raw, &destination).expect("place the raw archive under the snapshot id");
}

/// Seed one SQLite deployment, really erase the denied subject, initialize the
/// journal, and cut a fresh post-erasure archive under the live authority.
pub(super) fn gated_sqlite(scratch: &common::Scratch, backups: &Path, journal: &Path) -> Archive {
    let db = scratch.join("source/clio.db");
    let kms = scratch.join("source/clio.kms.json");
    seed_sqlite(
        &db,
        &kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );
    let store = open_sqlite(&db, &kms);
    erase_subject(&store, DENIED_SUBJECT, "req-scenario05-sqlite");
    drop(store);
    initialize_new_empty(journal).expect("initialize the deployment");
    let archive = archive_sqlite_with_authority(
        backups,
        &db,
        &kms,
        "sqlite-presence",
        Some(current_authority(journal)),
    );
    common::seed_restore_custody(journal, &archive.id, &archive.path);
    archive
}

/// A clean SQLite archive that references no denied subject but carries one
/// undeclared member, so only the unclassifiable member can refuse it.
pub(super) fn unknown_sqlite(scratch: &common::Scratch, backups: &Path, journal: &Path) -> Archive {
    let db = scratch.join("control/clio.db");
    let kms = scratch.join("control/clio.kms.json");
    seed_sqlite(
        &db,
        &kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let clean = archive_sqlite_with_authority(
        backups,
        &db,
        &kms,
        "sqlite-clean",
        Some(current_authority(journal)),
    );
    place_raw(
        backups,
        &clean.id,
        &raw_with_unclassifiable(&clean, &scratch.join("raw")),
    );
    clean
}

/// Seed one Postgres deployment, really erase the denied subject, initialize
/// the journal, and cut a fresh post-erasure archive under the live authority.
pub(super) fn gated_pg(
    base: &str,
    scratch: &common::Scratch,
    backups: &Path,
    journal: &Path,
) -> Archive {
    let source = ScopedSchema::new(base);
    let kms = scratch.join("source/clio.kms.json");
    seed_pg(
        source.url(),
        &kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );
    let store = open_pg(source.url(), &kms);
    erase_subject(&store, DENIED_SUBJECT, "req-scenario05-pg");
    drop(store);
    initialize_new_empty(journal).expect("initialize the deployment");
    let archive = archive_postgres_with_authority(
        backups,
        source.url(),
        &kms,
        "pg-presence",
        Some(current_authority(journal)),
    );
    common::seed_restore_custody(journal, &archive.id, &archive.path);
    archive
}

/// A clean Postgres archive that references no denied subject but carries one
/// undeclared member, so only the unclassifiable member can refuse it.
pub(super) fn unknown_pg(
    base: &str,
    scratch: &common::Scratch,
    backups: &Path,
    journal: &Path,
) -> Archive {
    let control = ScopedSchema::new(base);
    let kms = scratch.join("control/clio.kms.json");
    seed_pg(
        control.url(),
        &kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let clean = archive_postgres_with_authority(
        backups,
        control.url(),
        &kms,
        "pg-clean",
        Some(current_authority(journal)),
    );
    place_raw(
        backups,
        &clean.id,
        &raw_with_unclassifiable(&clean, &scratch.join("raw")),
    );
    clean
}
