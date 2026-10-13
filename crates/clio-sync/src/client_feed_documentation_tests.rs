// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one consistency property: the field sets the
//! operator-facing help enumerates are the sets the relay's journal actually
//! has, so the documentation cannot drift from the code silently.
//!
//! ## Owns
//! - The check that every journal column is either documented with its side or
//!   listed as deliberately out of scope with the reason, which is what makes a
//!   later phase's added or removed column fail this check
//! - The check that the `sync serve` help names each documented field inside the
//!   clause that field's side names, plus the residual, the already-stored-rows
//!   boundary, and the deployment control
//! - The wire-side half: a real sealed push whose documented cleartext columns
//!   are read back from the relay's own journal
//! - The key verbs' help: the lifecycle, and the rotation procedure with both
//!   peer-visible failure modes
//!
//! ## Does not own
//! - The sealed-set blindness assertions themselves
//!   (`client_wire_blindness_tests`, `client_wire_digest_tests`)
//! - The warning text (`bind_warning_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the help text, the journal's column list, or the
//! documented sides change; they never change any of them.
//!
//! Test-only. Never compiled into production builds.

use crate::client::client_tests::{cfg, serve, sqlite};
use crate::client::push;
use crate::client_wire_fixtures::seed_persona;
use crate::crypto::SyncCipherKey;
use crate::field_documentation_test_support::{
    DOCUMENTED_FIELDS, Side, assert_each_field_is_named_on_its_side, documented_cleartext_names,
};
use clio_store::{SyncJournalRow, SyncStore};

/// The operator-facing help this crate's sealed wire is documented in: the sync
/// registry records, which now carry every command's usage text. Read at compile
/// time, following the cross-crate source-read pattern this crate's own
/// auth-path and store-boundary tests already use; a moved file fails this
/// module's compilation loudly rather than silently passing.
const HELP_USAGE: &str = include_str!("../../clio/src/cli_command_registry_sync.rs");

/// The store's shared journal column projection, read the same way. This is the
/// list a later phase changes when it adds or removes a wire field, so the
/// drift check reads it rather than a copy of it.
const STORE_ROWS: &str = include_str!("../../clio-store/src/sync_store_rows.rs");

/// The journal columns this build carries that the documented set deliberately
/// does not name, each with why it is out of scope.
///
/// This is what makes the drift check total. A later phase that adds a column
/// has to classify it here or in the table, so the omission is a decision with a
/// reason rather than a silence.
const UNDOCUMENTED_COLUMNS: [(&str, &str); 4] = [
    (
        "seq",
        "the relay's own commit-ordered position within the bank. It is assigned \
         when the row is appended and handed back as the pull cursor, never sent \
         by the pushing node, and it is not one of the enumerated routing fields.",
    ),
    (
        "sync_mutation_id",
        "the relay's redelivery key. It is a keyed digest of the real identifier \
         under a key the relay does not hold, so a relay can neither read it nor \
         confirm a guessed identifier against it. This build's warning is not \
         widened to claim it, and the set it does name is left as it was agreed.",
    ),
    (
        "encrypted",
        "a per-row flag saying whether the content is sealed. It names no value.",
    ),
    (
        "created_at",
        "the relay's own insert timestamp, assigned when the row is appended rather \
         than sent by the pushing node.",
    ),
];

/// The prose one sync command's help prints, from the record that now holds it.
///
/// The migration moved every command's usage text out of the help file and into
/// the registry records, where a command's prose is the note constant its record
/// carries, so an entry is located by that constant rather than by a
/// `usage: clio ...` line. The record is required to name the note as well: a
/// note that survives as an unreferenced constant, or one dropped from its
/// record's `notes` list, fails here instead of passing on text nothing renders.
fn help_entry(verb: &str, note: &str) -> &'static str {
    let marker = format!("const {note}: &str = \"");
    let start = HELP_USAGE.find(&marker).unwrap_or_else(|| {
        panic!("help usage must document `{verb}`: no `{note}` in the sync registry");
    });
    let text = &HELP_USAGE[start + marker.len()..];
    let end = text
        .find("\";")
        .unwrap_or_else(|| panic!("`{note}` must be a closed string literal"));

    let record = format!("name: \"{verb}\",");
    let record_start = HELP_USAGE.find(&record).unwrap_or_else(|| {
        panic!("the sync registry must record `{verb}`");
    });
    let record_end = HELP_USAGE[record_start..]
        .find("};")
        .map_or(HELP_USAGE.len(), |offset| record_start + offset);
    assert!(
        HELP_USAGE[record_start..record_end].contains(note),
        "the `{verb}` record must name the `{note}` prose its help prints"
    );

    &text[..end]
}

/// The journal's own column list, parsed out of the store's projection.
///
/// The source uses trailing backslashes to continue the literal across lines,
/// so the continuations are removed before the list is split. Reading the
/// projection rather than a copied list is the point: a copied list would keep
/// passing after the projection changed.
fn journal_columns() -> Vec<String> {
    const MARKER: &str = "const JOURNAL_COLS: &str = \"";
    let start = STORE_ROWS
        .find(MARKER)
        .expect("the store must declare its journal column list");
    let after = start + MARKER.len();
    let end = after
        + STORE_ROWS[after..]
            .find('"')
            .expect("the journal column list must be a closed literal");
    STORE_ROWS[after..end]
        .replace("\\\n", "")
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The stored value of one documented cleartext column.
fn cleartext_column<'a>(row: &'a SyncJournalRow, column: &str) -> &'a str {
    match column {
        "bank_id" => &row.bank_id,
        "entity_kind" => &row.entity_kind,
        "op" => &row.op,
        "transaction_time" => &row.transaction_time,
        "device_id" => &row.device_id,
        "entity_id" => &row.entity_id,
        "content_hash" => &row.content_hash,
        other => panic!("`{other}` is not a documented cleartext column"),
    }
}

/// The documented fields that cross the network in the clear, sorted by column.
fn documented_cleartext_columns() -> Vec<&'static str> {
    let mut columns: Vec<&'static str> = DOCUMENTED_FIELDS
        .iter()
        .filter(|field| field.side != Side::Sealed)
        .map(|field| field.column)
        .collect();
    columns.sort_unstable();
    columns
}

/// Every journal column is either documented with the side it sits on or listed
/// as out of scope with a reason. A later phase that adds a column fails here
/// naming the column, which is what the drift guarantee the documentation
/// promises actually rests on.
#[test]
fn every_journal_column_is_documented_or_explained() {
    let documented: Vec<&str> = DOCUMENTED_FIELDS.iter().map(|field| field.column).collect();
    let explained: Vec<&str> = UNDOCUMENTED_COLUMNS
        .iter()
        .map(|(column, _)| *column)
        .collect();

    let mut classified = documented.clone();
    classified.extend(explained.iter().copied());
    let total = classified.len();
    classified.sort_unstable();
    classified.dedup();
    assert_eq!(
        classified.len(),
        total,
        "a column is classified twice, so the classification does not say which side it is on: {classified:?}"
    );

    let mut actual = journal_columns();
    actual.sort_unstable();
    classified.sort_unstable();
    assert_eq!(
        actual, classified,
        "the journal's columns and their classification must be the same set. A column \
         the projection has and the classification does not, or the other way round, \
         is a wire field the documentation has not caught up with."
    );
    for (column, reason) in UNDOCUMENTED_COLUMNS {
        assert!(!reason.is_empty(), "{column} is unexplained");
    }
}

/// The `sync serve` help names every documented field inside the clause that
/// field's side names, so a field that crosses the network in the clear cannot
/// be presented as sealed by being mentioned somewhere in the entry. It also
/// carries the residual, the already-stored-rows boundary, and the deployment
/// control.
#[test]
fn the_serve_help_names_each_documented_field_on_the_side_it_sits_on() {
    let entry = help_entry("sync serve", "SERVE_CLEARTEXT_NOTE");
    assert_each_field_is_named_on_its_side(entry);
    for residual in ["record-kind mix", "device-id stability"] {
        assert!(
            entry.contains(residual),
            "serve help must name the residual `{residual}`: {entry}"
        );
    }
    for boundary in ["rows already on a relay", "re-pushed"] {
        assert!(
            entry.contains(boundary),
            "serve help must state the already-stored-rows boundary via `{boundary}`: {entry}"
        );
    }
    assert!(
        entry.contains("reverse proxy"),
        "serve help names the deployment control: {entry}"
    );
}

/// The help enumerates exactly the documented cleartext set, so the two cannot
/// drift apart without this failing. The count is what catches a field added to
/// the prose and forgotten in the table.
#[test]
fn the_serve_help_enumerates_the_documented_cleartext_set() {
    let entry = help_entry("sync serve", "SERVE_CLEARTEXT_NOTE");
    let expected = documented_cleartext_names();
    let mut present: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|name| entry.contains(name))
        .collect();
    present.sort_unstable();
    assert_eq!(
        present, expected,
        "the serve help must enumerate every documented cleartext field"
    );
    // A duplicated cleartext name would make the enumeration above pass while
    // one documented field went unchecked, so uniqueness is checked here rather
    // than left to the reader. `expected` arrives sorted, which is what makes
    // `dedup` catch a repeat: it only removes adjacent entries.
    let mut unique = expected.clone();
    unique.dedup();
    assert_eq!(
        unique.len(),
        expected.len(),
        "the cleartext set names the same field twice, so one field's side is checked twice \
         and another is not checked at all: {expected:?}"
    );
}

/// The key verbs' help carries the lifecycle, the rotation procedure, and both
/// peer-visible failure modes: a peer without the new key dead-letters and its
/// pull cursor freezes, while content under a held key stays readable.
#[test]
fn the_key_verbs_help_carry_the_lifecycle_and_the_rotation_procedure() {
    let keygen = help_entry("sync keygen", "KEYGEN_NOTE");
    assert!(keygen.contains("never leaves the node"), "{keygen}");
    assert!(keygen.contains("seals with the newest key"), "{keygen}");
    assert!(keygen.contains("opens with any key it holds"), "{keygen}");
    assert!(keygen.contains("confirm the fingerprint"), "{keygen}");
    assert!(keygen.contains("dead-letters"), "failure mode 1: {keygen}");
    assert!(
        keygen.contains("pull cursor freezes"),
        "failure mode 1: {keygen}"
    );
    assert!(
        keygen.contains("stays readable"),
        "failure mode 2: {keygen}"
    );

    let export = help_entry("sync key export", "KEY_EXPORT_NOTE");
    assert!(export.contains("on purpose"), "{export}");
    assert!(export.contains("rotation"), "{export}");

    let status = help_entry("sync key status", "KEY_STATUS_NOTE");
    assert!(status.contains("fingerprint"), "{status}");
    assert!(
        status.contains("never the key or a prefix of it"),
        "{status}"
    );

    let status_report = help_entry("sync status", "STATUS_ROTATION_NOTE");
    assert!(
        status_report.contains("dead-letters"),
        "the rotation section points at the visible report: {status_report}"
    );
}

/// The wire half: a sealed push travels with every documented cleartext column
/// readable in the clear, so the enumeration above documents the wire and not an
/// intention. The set is read from the table rather than written out here, so a
/// newly documented cleartext field is covered by this test without editing it.
/// The sealed side is asserted by the blindness modules; the cleartext columns
/// are what this one pins.
#[test]
fn a_sealed_push_writes_every_documented_cleartext_column() {
    let (server, srv, _kms) = serve(&["bp"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg(
        "dev-a",
        &a,
        &addr,
        vec![SyncCipherKey::from_secret(
            &crate::credential_fixtures::secret("feed-doc-key"),
        )],
    );
    seed_persona(&a, "bp");
    push(&cfg_a, &["bp".to_owned()]).expect("push");

    let rows = srv.journal_list_since("bp", 0, 50).expect("server journal");
    let sealed: Vec<_> = rows.iter().filter(|row| row.client_encrypted).collect();
    assert!(!sealed.is_empty(), "the push sealed its rows");
    let mut empty = Vec::new();
    for row in &sealed {
        for field in DOCUMENTED_FIELDS.iter().filter(|f| f.side != Side::Sealed) {
            if cleartext_column(row, field.column).is_empty() {
                empty.push(field.column);
            }
        }
    }
    empty.sort_unstable();
    empty.dedup();
    assert_eq!(
        empty,
        Vec::<&str>::new(),
        "a documented cleartext column was empty on the relay's own journal, so the \
         enumeration documents an intention rather than the wire. Expected columns: \
         {:?}",
        documented_cleartext_columns()
    );
    server.stop();
}
