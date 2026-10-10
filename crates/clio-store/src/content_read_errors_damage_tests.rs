// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving every way a stored item row can be
//! damaged produces `UnreadableContent` on both backends.
//!
//! ## Owns
//! - The table of damage shapes and the code each one must report
//! - The same table run against live SQLite and live Postgres
//!
//! ## Does not own
//! - Which code a read produces in production (`dek`, `item_persist`)
//! - Whether a caller skips the row (`content_read_errors`)
//! - The admission write path (`clio-mcp` signal-source tests)
//!
//! # Why the table runs twice
//!
//! The two backends have separate read paths (`sqlite_memory`, `postgres_memory`)
//! but share the decode chain, so a shape fixed in one and not the other is
//! invisible to a single-backend test. One table, two backends, one assertion
//! each: a shape that regresses anywhere fails here.
//!
//! # Why the codes are asserted, not just the failure
//!
//! The optional-row readers skip on `UnreadableContent` and fail closed on
//! `Internal`. A shape that reported `Internal` would still fail this read — and
//! would still pass a test that only checked `is_err()` — while making one
//! damaged row abort every write in the bank. So each row names the code.

use crate::content_read_errors::is_unreadable_content;
use crate::dek::{CiphertextEnvelope, DekProvider, LocalDevKms};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{AmError, EpistemicKind, ErrorCode, ItemKind, MemoryItem, SemanticCategory};
use std::sync::Arc;

/// One way a stored item row can be damaged.
struct Damage {
    /// What was broken, for the assertion message.
    name: &'static str,
    /// Rewrites the sealed column. `None` keeps the envelope's own fields and
    /// re-serializes the mutated envelope; `Some(text)` replaces the whole
    /// column, which is how a row that is not an envelope at all is produced.
    tamper: fn(&mut CiphertextEnvelope) -> Option<String>,
    /// The code `get_memory_item` must report.
    code: ErrorCode,
}

/// Base64 of three bytes: valid base64, wrong nonce length.
fn short_nonce() -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0u8; 3])
}

/// The five damage shapes a single item row can present to a reader.
const DAMAGES: &[Damage] = &[
    Damage {
        name: "undecodable ciphertext",
        tamper: |e| {
            e.ciphertext_b64 = "!!!not-base64!!!".to_owned();
            None
        },
        code: ErrorCode::UnreadableContent,
    },
    Damage {
        name: "undecodable nonce",
        tamper: |e| {
            e.nonce_b64 = "****".to_owned();
            None
        },
        code: ErrorCode::UnreadableContent,
    },
    Damage {
        name: "wrong nonce length",
        tamper: |e| {
            e.nonce_b64 = short_nonce();
            None
        },
        code: ErrorCode::UnreadableContent,
    },
    Damage {
        name: "content column is not an envelope",
        tamper: |_| Some("this-is-not-an-envelope".to_owned()),
        code: ErrorCode::UnreadableContent,
    },
    Damage {
        name: "empty content column",
        tamper: |_| Some(String::new()),
        code: ErrorCode::UnreadableContent,
    },
];

fn sample_item(id: &str, bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: Some("a note worth indexing".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Assert every shape in the table against one store, then confirm the store
/// still works for an undamaged row.
///
/// `exec` runs the tamper SQL. It is a parameter rather than a trait method
/// because the raw-statement escape hatch is inherent to each backend, not part
/// of [`Store`].
fn assert_damage_table(
    store: &dyn Store,
    exec: &dyn Fn(&str) -> Result<(), AmError>,
    bank: &str,
    label: &str,
) {
    let kms = LocalDevKms::new();

    for (index, damage) in DAMAGES.iter().enumerate() {
        let id = format!("{label}-damaged-{index}");
        store
            .create_memory_item(&sample_item(&id, bank, "subject-damage"), "actor")
            .expect("seed item");

        // The row is stored normally, then damaged the way storage damage would
        // damage it: the sealed column is rewritten in place.
        let mut envelope = kms.encrypt("subject-damage", b"sealed bytes").unwrap();
        let replacement = match (damage.tamper)(&mut envelope) {
            Some(text) => text,
            None => envelope.to_storage_string(),
        };
        assert!(
            !replacement.contains('\''),
            "the SQL literal must not need quote escaping"
        );
        exec(&format!(
            "UPDATE items SET content_ciphertext = '{replacement}' WHERE id = '{id}'"
        ))
        .expect("tamper");

        let err = store
            .get_memory_item(bank, &id)
            .expect_err("a damaged row must not open");
        assert_eq!(
            err.code, damage.code,
            "{label}: {} must report {:?}, got {err}",
            damage.name, damage.code
        );
        assert!(
            is_unreadable_content(&err),
            "{label}: {} must be skippable by an optional-row reader",
            damage.name
        );
    }

    // A NULL column is the same missing-content condition as an empty one.
    let null_id = format!("{label}-damaged-null");
    store
        .create_memory_item(&sample_item(&null_id, bank, "subject-damage"), "actor")
        .expect("seed item");
    exec(&format!(
        "UPDATE items SET content_ciphertext = NULL WHERE id = '{null_id}'"
    ))
    .expect("null out");
    assert_eq!(
        store
            .get_memory_item(bank, &null_id)
            .expect_err("a NULL content column must not open")
            .code,
        ErrorCode::UnreadableContent,
        "{label}: NULL content column"
    );

    // Control: an undamaged row in the same store still reads back.
    let clean_id = format!("{label}-clean");
    store
        .create_memory_item(&sample_item(&clean_id, bank, "subject-clean"), "actor")
        .expect("seed item");
    assert!(
        store
            .get_memory_item(bank, &clean_id)
            .expect("clean read")
            .is_some(),
        "{label}: an undamaged row must still open"
    );
}

#[test]
fn sqlite_reports_every_damage_shape_as_unreadable_content() {
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = SqliteStore::open_in_memory(kms).expect("store");
    assert_damage_table(
        &store,
        &|sql| store.test_execute(sql),
        "bank-sqlite-damage",
        "sqlite",
    );
}

#[test]
fn postgres_reports_every_damage_shape_as_unreadable_content() {
    let _guard = crate::lock_pg_test();
    let scoped = crate::ScopedSchema::new(&crate::db_url::postgres_url_from_env());
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let store = PostgresStore::open(scoped.url(), Arc::clone(&kms)).expect("store");
    assert_damage_table(
        &store,
        &|sql| store.test_execute(sql),
        "bank-pg-damage",
        "pg",
    );
}

/// The classification must not be reachable by accident: an error the predicate
/// rejects is one an optional-row reader will fail closed on.
#[test]
fn a_backend_error_is_not_mistaken_for_damage() {
    let broken = AmError::new(ErrorCode::Internal, "items: file is not a database");
    assert!(
        !is_unreadable_content(&broken),
        "a broken database must stay fail-closed"
    );
}
