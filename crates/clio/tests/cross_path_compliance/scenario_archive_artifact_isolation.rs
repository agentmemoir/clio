// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that each artifact kind a
//! post-erasure archive carries for an erased subject is a presence the
//! restore authority check counts on its own: the subject's tombstone, its
//! shredded subject key, its destroyed key slot, and its ciphertext-only item
//! row each refuse the whole archive when they are the only artifact left in
//! it, so none of them can be carried by the others.
//!
//! It also measures the one artifact kind the check reads from the payload's
//! item table: the subject's ciphertext-only item row. The authority check
//! reads `compliance_tombstones`, `subject_keys`, and `items` in the payload
//! and `slots` in the key member, so an archive carrying that row alone names
//! the denied subject and is refused as a whole, before anything is placed.
//!
//! ## Owns
//! - The single-artifact archives and the artifact kind each one carries alone
//! - The per-artifact refusal assertion and the unchanged-target check
//!
//! ## Does not own
//! - The whole-archive presence case and the unclassifiable member (see
//!   `scenario_archive_presence`)
//! - The archive assembly and the read-back probes (see `support_test_support`
//!   and `support_pg_test_support`)
//!
//! ## Boundary
//!
//! Changes to which artifact kinds count as presence should normally require
//! changes here. Changes to the archive format should not.
//!
//! Keep this module focused on the one-artifact-at-a-time isolation.

use crate::case_fixtures::{
    BANK, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, backup_dir, journal_file,
    open_pg, open_sqlite, scratch, seed_pg, seed_sqlite, unique,
};
use crate::cli_test_support::{DENIED_SUBJECT_REFUSAL, assert_refused, denied_hash, run_cli};
use crate::common;
use crate::support_pg_test_support::{
    archive_postgres_with_authority, lock_pg, pg_url, postgres_reachable, require_postgres,
};
use crate::support_test_support::{
    Archive, archive_sqlite_with_authority, current_authority, erase_subject, file_sha256, items,
};
use clio::store::ScopedSchema;
use clio_compliance::{initialize_new_empty, record_denial};
use rusqlite::Connection;
use std::path::Path;

/// One artifact kind a post-erasure archive carries for the erased subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Artifact {
    /// The subject's tombstone row in `compliance_tombstones`.
    Tombstone,
    /// The subject's shredded row in `subject_keys`.
    SubjectKey,
    /// The subject's destroyed slot in the key store the archive carries.
    KeySlot,
    /// The subject's ciphertext-only row in `items`.
    ItemRow,
}

/// The artifact kinds the archive-kind loop drives, in the order the cases
/// drive them. Each one refuses the whole archive when it is left alone. The
/// ciphertext-only item row is driven by its own arm.
const REFUSING_ARTIFACTS: [Artifact; 3] =
    [Artifact::Tombstone, Artifact::SubjectKey, Artifact::KeySlot];

/// The artifact kinds the store itself holds, in the order they are removed.
const STORE_ARTIFACTS: [Artifact; 3] =
    [Artifact::Tombstone, Artifact::SubjectKey, Artifact::ItemRow];

impl Artifact {
    /// The kind's short name, used in the per-case paths and the messages.
    fn tag(self) -> &'static str {
        match self {
            Self::Tombstone => "tombstone",
            Self::SubjectKey => "subject-key",
            Self::KeySlot => "key-slot",
            Self::ItemRow => "item-row",
        }
    }
}

/// The SQL that removes the erased subject's rows of `kind`.
///
/// The values are this fixture's own subject id and subject hash, both built
/// in this file, so the statement carries no operator input.
fn drop_sql(kind: Artifact) -> String {
    let subject = DENIED_SUBJECT;
    match kind {
        Artifact::Tombstone => format!(
            "DELETE FROM compliance_tombstones WHERE subject_hash = '{}'",
            denied_hash()
        ),
        Artifact::SubjectKey => format!("DELETE FROM subject_keys WHERE subject_id = '{subject}'"),
        Artifact::ItemRow => format!("DELETE FROM items WHERE subject_id = '{subject}'"),
        Artifact::KeySlot => format!("DELETE FROM slots WHERE subject_id = '{subject}'"),
    }
}

/// Remove every store-held artifact kind but `keep` from the erased SQLite
/// store, so the next archive carries `keep` alone.
fn isolate_store(connection: &Connection, keep: Artifact) {
    for kind in STORE_ARTIFACTS {
        if kind == keep {
            continue;
        }
        connection
            .execute_batch(&drop_sql(kind))
            .expect("remove one artifact kind from the erased store");
    }
}

/// Remove every store-held artifact kind but `keep` from the erased Postgres
/// schema, so the next archive carries `keep` alone.
fn isolate_schema(schema: &ScopedSchema, keep: Artifact) {
    for kind in STORE_ARTIFACTS {
        if kind == keep {
            continue;
        }
        schema
            .execute(&drop_sql(kind))
            .expect("remove one artifact kind from the erased schema");
    }
}

/// Remove the erased subject's destroyed key slot from the key store, unless
/// the key slot is the one artifact the archive must carry alone.
fn isolate_key_store(kms: &Path, keep: Artifact) {
    if keep == Artifact::KeySlot {
        return;
    }
    let connection = Connection::open(kms).expect("open the key store");
    connection
        .execute_batch(&drop_sql(Artifact::KeySlot))
        .expect("remove the destroyed key slot");
}

/// Seed one SQLite source store, really erase the denied subject, keep only
/// `kind`'s artifact, and cut the archive that carries it alone.
fn sqlite_archive(
    scratch: &common::Scratch,
    backups: &Path,
    journal: &Path,
    kind: Artifact,
) -> Archive {
    let source = scratch.join(format!("source-{}/clio.db", kind.tag()));
    let kms = scratch.join(format!("source-{}/clio.kms.json", kind.tag()));
    seed_sqlite(
        &source,
        &kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );
    let store = open_sqlite(&source, &kms);
    erase_subject(&store, DENIED_SUBJECT, "req-archive-artifacts-sqlite");
    drop(store);
    {
        let connection = Connection::open(&source).expect("open the erased store");
        isolate_store(&connection, kind);
    }
    isolate_key_store(&kms, kind);
    archive_sqlite_with_authority(
        backups,
        &source,
        &kms,
        &unique("artifact"),
        Some(current_authority(journal)),
    )
}

/// Seed one Postgres source schema, really erase the denied subject, keep only
/// `kind`'s artifact, and cut the archive that carries it alone.
fn postgres_archive(
    base: &str,
    scratch: &common::Scratch,
    backups: &Path,
    journal: &Path,
    kind: Artifact,
) -> Archive {
    let source = ScopedSchema::new(base);
    let kms = scratch.join(format!("source-{}/clio.kms.json", kind.tag()));
    seed_pg(
        source.url(),
        &kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );
    let store = open_pg(source.url(), &kms);
    erase_subject(&store, DENIED_SUBJECT, "req-archive-artifacts-pg");
    drop(store);
    isolate_schema(&source, kind);
    isolate_key_store(&kms, kind);
    archive_postgres_with_authority(
        backups,
        source.url(),
        &kms,
        &unique("artifact"),
        Some(current_authority(journal)),
    )
}

/// The SQLite arm: one archive per artifact kind, each carrying one kind
/// alone, and each refused as a whole before it places anything.
#[test]
fn sqlite_each_artifact_kind_alone_refuses_the_whole_archive() {
    let scratch = scratch("scenario-artifacts-sqlite");
    let backups = backup_dir(&scratch);
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target = target_db
        .to_str()
        .expect("the target path is UTF-8")
        .to_owned();
    let target_before = file_sha256(&target_db);

    for kind in REFUSING_ARTIFACTS {
        let archive = sqlite_archive(&scratch, &backups, &journal, kind);
        let output = run_cli(
            &journal,
            &target_kms,
            &backups,
            &[
                "backup",
                "restore",
                archive.id.as_str(),
                "--yes",
                "--db",
                target.as_str(),
                "--output",
                "json",
            ],
        );
        assert_refused(&output, &DENIED_SUBJECT_REFUSAL);
        assert_eq!(
            file_sha256(&target_db),
            target_before,
            "the {} archive placed bytes",
            kind.tag()
        );
    }

    // The ciphertext-only item row keeps the denied subject's link, so the
    // archive carrying that row alone names the subject and is refused as a
    // whole, before anything is placed.
    let archive = sqlite_archive(&scratch, &backups, &journal, Artifact::ItemRow);
    let output = run_cli(
        &journal,
        &target_kms,
        &backups,
        &[
            "backup",
            "restore",
            archive.id.as_str(),
            "--yes",
            "--db",
            target.as_str(),
            "--output",
            "json",
        ],
    );
    assert_refused(&output, &DENIED_SUBJECT_REFUSAL);
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the item-row archive placed bytes"
    );

    println!("REFUSED sqlite artifacts=4 item_row=refused");
}

/// The Postgres arm: the same isolation, against the dump's own tables.
#[test]
fn postgres_each_artifact_kind_alone_refuses_the_whole_archive() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let scratch = scratch("scenario-artifacts-pg");
    let backups = backup_dir(&scratch);
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let target = ScopedSchema::new(&base);
    let target_kms = scratch.join("target/clio.kms.json");
    seed_pg(
        target.url(),
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target_url = target.url().to_owned();
    let items_before = items(&target_url);

    for kind in REFUSING_ARTIFACTS {
        let archive = postgres_archive(&base, &scratch, &backups, &journal, kind);
        let output = run_cli(
            &journal,
            &target_kms,
            &backups,
            &[
                "backup",
                "restore",
                archive.id.as_str(),
                "--yes",
                "--db",
                target_url.as_str(),
                "--output",
                "json",
            ],
        );
        assert_refused(&output, &DENIED_SUBJECT_REFUSAL);
        assert_eq!(
            items(&target_url),
            items_before,
            "the {} archive placed rows",
            kind.tag()
        );
    }

    // The ciphertext-only item row keeps the denied subject's link, so the
    // archive carrying that row alone names the subject and is refused as a
    // whole, before anything is placed.
    let archive = postgres_archive(&base, &scratch, &backups, &journal, Artifact::ItemRow);
    let output = run_cli(
        &journal,
        &target_kms,
        &backups,
        &[
            "backup",
            "restore",
            archive.id.as_str(),
            "--yes",
            "--db",
            target_url.as_str(),
            "--output",
            "json",
        ],
    );
    assert_refused(&output, &DENIED_SUBJECT_REFUSAL);
    assert_eq!(
        items(&target_url),
        items_before,
        "the item-row archive placed rows"
    );

    println!("REFUSED postgres artifacts=4 item_row=refused");
}
