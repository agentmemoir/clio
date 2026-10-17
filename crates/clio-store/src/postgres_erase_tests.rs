// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that Postgres opaque writes refuse erased subjects.
//!
//! ## Owns
//! - Postgres regression coverage for opaque writes after subject erasure
//!
//! ## Does not own
//! - Postgres erasure SQL (`postgres_erase`)
//! - Opaque item upsert SQL (`opaque_item_persist`)
//!
//! ## Boundary
//!
//! Changes to the Postgres erased-subject opaque write contract should normally require changes here.
//! Changes to erasure fixtures should require `erase_test_support`.
//!
//! Keep this module focused on the Postgres opaque write erasure contract.
//!
//! Test-only. Never compiled into production builds.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::compliance_store::ComplianceStore;
use crate::db_url::postgres_url_from_env;
use crate::erase_test_support::{local_kms, memory_item, opaque_write, spec, unique};
use crate::migrate::IntoAmError;
use crate::model::OpaqueItemRecord;
use crate::pg_lock::lock_pg_test;
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

#[test]
fn opaque_item_write_for_erased_subject_returns_error_without_creating_row() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = local_kms();
    let store = match PostgresStore::open(url.as_str(), Arc::clone(&kms)) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("opaque item erasure guard", &err);
            return;
        }
    };
    let bank = unique("opaque-erased-pg-bank");
    let subject = unique("opaque-erased-pg-subject");
    let item_id = unique("opaque-erased-pg-seeded-item");
    let opaque_item_id = unique("opaque-erased-pg-new-item");
    store
        .create_memory_item(&memory_item(&bank, &item_id, &subject), "tester")
        .unwrap();
    let cipher = store
        .inspect_content_ciphertext(&bank, &item_id)
        .unwrap()
        .unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("opaque-erased-pg-request")))
        .unwrap();

    let result = store.apply_batch(&[BatchWrite::PutOpaqueItem {
        item: OpaqueItemRecord {
            id: opaque_item_id.clone(),
            bank_id: bank.clone(),
            subject_id: subject.clone(),
            kind: "semantic".into(),
            category: Some("tool_config".into()),
            episodic_type: None,
            epistemic_kind: "fact".into(),
            source_type: None,
            confidence: Some(0.8),
            admission_score: Some(0.0),
            source_ref: Some("turn_9".into()),
            valid_from: "t0".into(),
            valid_until: None,
            tx_from: "t0".into(),
            tx_until: None,
            created_at: "t0".into(),
            updated_at: "t0".into(),
            content_cipher: cipher,
        },
        actor: "import".into(),
    }]);
    let opaque_row_count = store
        .with_client(|client| {
            Ok(client
                .query_one(
                    "SELECT COUNT(*) FROM items WHERE id = $1",
                    &[&opaque_item_id],
                )
                .am("read opaque item row count")?
                .get::<_, i64>(0))
        })
        .unwrap();
    assert!(
        matches!(&result, Err(err) if err.code == ErrorCode::ErasedSubject),
        "opaque write result: {result:?}; new row count: {opaque_row_count}"
    );
    assert_eq!(opaque_row_count, 0, "refused opaque item has no row");
    assert_eq!(
        store.get_memory_item(&bank, &item_id).unwrap_err().code,
        ErrorCode::ErasedSubject,
        "erased item content stays unreadable"
    );
    store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}';"
        ))
        .unwrap();
}

/// The per-subject advisory lock is exclusive, so an erase and an opaque write
/// for the same subject cannot interleave: both take this exact lock first, so
/// exclusion here means serialization there.
///
/// Determinism: `pg_try_advisory_xact_lock` returns a definite boolean without
/// waiting, so the test reads the lock state directly instead of racing a clock
/// or a timeout. One transaction takes the production lock; a second asks for
/// the same key and must be refused; after the holder commits the key is free.
#[test]
fn postgres_subject_write_lock_is_exclusive_while_held() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let Ok(mut holder) = crate::pg_tls::connect(&url) else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let Ok(mut probe) = crate::pg_tls::connect(&url) else {
        eprintln!("skip: shared Postgres unreachable");
        return;
    };
    let subject = unique("pg-lock-subject");

    let mut hold_tx = holder.transaction().am("holder begin").unwrap();
    crate::postgres_memory::lock_subject_write_tx(&mut hold_tx, &subject).unwrap();

    let mut probe_tx = probe.transaction().am("probe begin").unwrap();
    let held: bool = probe_tx
        .query_one(
            "SELECT pg_try_advisory_xact_lock(hashtext('subject-key:' || $1))",
            &[&subject],
        )
        .am("probe try-lock")
        .unwrap()
        .get(0);
    assert!(!held, "the subject lock is exclusive while held");
    probe_tx.rollback().am("probe rollback").unwrap();

    hold_tx.commit().am("holder commit").unwrap();

    let mut after_tx = probe.transaction().am("probe after begin").unwrap();
    let held_after: bool = after_tx
        .query_one(
            "SELECT pg_try_advisory_xact_lock(hashtext('subject-key:' || $1))",
            &[&subject],
        )
        .am("probe try-lock after release")
        .unwrap()
        .get(0);
    assert!(held_after, "the subject lock frees when the holder commits");
    after_tx.rollback().am("probe after rollback").unwrap();
}

/// A bounded writer-vs-eraser race: one thread streams opaque writes for a
/// subject while the main thread erases it. Whatever the interleaving, no row
/// for the erased subject may stay readable once erasure has completed, and a
/// refused write must leave no row behind.
///
/// Determinism: no sleep and no clock. A barrier starts both sides together and
/// a channel plus `join` end the test only after every write has reported, so
/// the assertion sees a complete interleaving rather than a partial one. The
/// invariant must hold under every schedule, which is what makes a single run
/// meaningful.
#[test]
fn postgres_writer_and_eraser_leave_no_readable_row_for_the_erased_subject() {
    let _guard = lock_pg_test();
    let url = postgres_url_from_env();
    let kms = local_kms();
    let main_store = match PostgresStore::open(url.as_str(), Arc::clone(&kms)) {
        Ok(store) => store,
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("writer vs eraser race", &err);
            return;
        }
    };
    // Open the writer's connection before the race so no schema bootstrap runs
    // while the erasure transaction holds locks.
    let writer_store = PostgresStore::open(url.as_str(), Arc::clone(&kms)).unwrap();

    let bank = unique("pg-race-bank");
    let subject = unique("pg-race-subject");
    let seed_id = unique("pg-race-seed");
    main_store
        .create_memory_item(&memory_item(&bank, &seed_id, &subject), "tester")
        .unwrap();
    let cipher = main_store
        .inspect_content_ciphertext(&bank, &seed_id)
        .unwrap()
        .unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Vec<(String, Result<(), AmError>)>>();
    let writer_barrier = Arc::clone(&barrier);
    let writer_bank = bank.clone();
    let writer_subject = subject.clone();
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        let mut results = Vec::new();
        for index in 0..32 {
            let id = format!("{writer_bank}-race-{index}");
            let result = writer_store.apply_batch(&[opaque_write(
                &writer_bank,
                &id,
                &writer_subject,
                &cipher,
            )]);
            results.push((id, result));
        }
        done_tx.send(results).expect("send writer results");
    });

    barrier.wait();
    main_store
        .erase_subject(&spec(&bank, &subject, &unique("pg-race-request")))
        .unwrap();
    let writes = done_rx.recv().expect("writer results");
    writer.join().expect("join writer thread");

    for (id, result) in &writes {
        match result {
            Ok(()) => assert!(
                !matches!(main_store.get_memory_item(&bank, id), Ok(Some(_))),
                "a refused-but-accepted row stayed readable: {id}"
            ),
            Err(err) => {
                assert_eq!(err.code, ErrorCode::ErasedSubject, "write {id}: {err}");
                let rows = main_store
                    .with_client(|client| {
                        client
                            .query_one("SELECT COUNT(*) FROM items WHERE id = $1", &[id])
                            .am("refused row count")
                            .map(|row| row.get::<_, i64>(0))
                    })
                    .unwrap();
                assert_eq!(rows, 0, "a refused write stored a row: {id}");
            }
        }
    }

    main_store
        .test_execute(&format!(
            "DELETE FROM index_pending WHERE bank_id = '{bank}'; \
             DELETE FROM items WHERE bank_id = '{bank}'; \
             DELETE FROM subject_keys WHERE subject_id = '{subject}'; \
             DELETE FROM compliance_tombstones WHERE bank_id = '{bank}';"
        ))
        .unwrap();
}
