// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the lexical query path never creates key material:
//! a search resolves the store's blind-index key read-only, and when the key
//! store is absent the search fails loudly and leaves the filesystem unchanged.
//!
//! ## Owns
//! - The regression for the read-path key lookup (no create, loud failure)
//!
//! ## Does not own
//! - The key provider contract under test (`dek`)
//! - The SQLite index driver (`sqlite_index`)
//!
//! ## Boundary
//!
//! Tests here break when a read path starts creating key material or turns a
//! missing key into a silent empty result; they never change that contract.
//!
//! Test-only. Never compiled into production builds.

use crate::dek::{DekProvider, LocalDevKms};
use crate::erase_test_support::{memory_item, unique};
use crate::index_store::IndexStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The key-store file name next to a scratch database.
fn kms_file(dir: &Path) -> PathBuf {
    dir.join("clio.kms.json")
}

#[test]
fn search_against_a_missing_key_store_fails_loudly_and_creates_nothing() {
    // Sealing uses the process test key, so this test never reaches the
    // machine's real key store: a developer machine with both a keystore entry
    // and a key file makes every seal fail with the two-store refusal, which
    // has nothing to do with what this test checks.
    crate::kms_migrate::install_test_key();
    let dir = crate::test_unique::scratch_dir("lexical-query-key");
    let db = dir.join("clio.db");
    let kms_path = kms_file(&dir);
    let bank = unique("qk-bank");
    let item = unique("qk-item");

    // Index one item through the write path with a file-backed key store.
    // Creating the key here is expected: this is the write path.
    {
        let kms = LocalDevKms::open_persistent(&kms_path).expect("open key store");
        let store =
            SqliteStore::open(&db, Arc::new(kms) as Arc<dyn DekProvider>).expect("open store");
        store
            .create_memory_item(&memory_item(&bank, &item, "subj-qk"), "tester")
            .expect("create item");
        IndexStore::put_lexical_doc(&store, &bank, &item, "wombat zephyr").expect("index the item");
        assert!(kms_path.exists(), "the write path created the key store");
    }

    // Lose the key store, then search again the way a fresh process would.
    std::fs::remove_file(&kms_path).expect("remove the key store");

    // Opening a missing key store is lazy: this must not create the file yet.
    let kms = LocalDevKms::open_persistent(&kms_path).expect("open the missing key store lazily");
    let store =
        SqliteStore::open(&db, Arc::new(kms) as Arc<dyn DekProvider>).expect("reopen the store");

    // The search must fail loudly rather than return an empty result set: the
    // query cannot derive the digests it has to match.
    let err = IndexStore::lexical_search(&store, &bank, "wombat", &[], 5)
        .expect_err("a search without the blind-index key fails");
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "error: {err}");

    // And the failed search must not have created key material: the pre-fix
    // behaviour created the file, and a backup then reported key material it
    // cannot use. (The store file itself may be touched by opening it, which
    // SQLite is free to do; the key store must not reappear.)
    assert!(
        dir.read_dir()
            .expect("list the scratch directory")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .all(|name| !name.contains("kms")),
        "a failed search must not create any key-store file"
    );

    std::fs::remove_dir_all(&dir).expect("clean the scratch directory");
}

#[test]
fn search_after_the_write_path_sees_the_same_key_without_creating_it() {
    // Hermetic like its siblings: sealing never reaches the machine's key store.
    crate::kms_migrate::install_test_key();
    // The normal order: an indexed store, a later search. The read-only
    // lookup must resolve the slot the write path created, so the two sides
    // meet without the query path ever creating key material.
    let dir = crate::test_unique::scratch_dir("lexical-query-key-normal");
    let db = dir.join("clio.db");
    let kms_path = kms_file(&dir);
    let bank = unique("qk-bank-ok");
    let item = unique("qk-item-ok");

    let kms = LocalDevKms::open_persistent(&kms_path).expect("open key store");
    let store = SqliteStore::open(&db, Arc::new(kms) as Arc<dyn DekProvider>).expect("open store");
    store
        .create_memory_item(&memory_item(&bank, &item, "subj-qk"), "tester")
        .expect("create item");
    IndexStore::put_lexical_doc(&store, &bank, &item, "wombat zephyr").expect("index the item");

    let hits = IndexStore::lexical_search(&store, &bank, "wombat", &[], 5).expect("search");
    assert_eq!(hits.len(), 1, "the indexed item is found");

    std::fs::remove_dir_all(&dir).expect("clean the scratch directory");
}

#[test]
fn search_on_an_empty_index_answers_empty_without_touching_key_material() {
    // A store that was written to but never indexed has no lexical rows: the
    // honest answer is an empty result, and no key is needed to prove it, so
    // no key-store file may appear either.
    let dir = crate::test_unique::scratch_dir("lexical-query-key-empty");
    let db = dir.join("clio.db");
    let kms_path = kms_file(&dir);
    let bank = unique("qk-bank-empty");
    let item = unique("qk-item-empty");

    let store = SqliteStore::open(&db, Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("open store");
    store
        .create_memory_item(&memory_item(&bank, &item, "subj-qk"), "tester")
        .expect("create item");

    let hits = IndexStore::lexical_search(&store, &bank, "wombat", &[], 5).expect("empty search");
    assert!(hits.is_empty(), "an empty index matches nothing");
    assert!(
        !kms_path.exists(),
        "an empty-index search must not create a key store"
    );

    std::fs::remove_dir_all(&dir).expect("clean the scratch directory");
}
