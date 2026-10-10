// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the security evidence of the lexical index:
//! no item word is readable from the stored column or from the store file, and
//! a dictionary of common words cannot be matched against the stored digests.
//!
//! ## Owns
//! - Needle-absence tests for `items_fts.content_text` and `items.content_tsv`
//! - Raw file and column scans, and the keyed-versus-bare-hash check
//!
//! ## Does not own
//! - The keyed encoding itself (`lexical_digest`)
//! - Archive inspection (the `clio backup` surface)
//!
//! ## Boundary
//!
//! Tests here break when a plaintext word can reach the stored lexical form;
//! they never weaken that bar.
//!
//! Keep this module focused on the security evidence.

use crate::dek::{DekProvider, LocalDevKms};
use crate::index_store::IndexStore;
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A word that must never reach a persisted byte anywhere.
const NEEDLE: &str = "zwibblequantumfox";

/// The indexed document. Every word falls outside `[0-9a-f]` on purpose, so a
/// readable word could never hide inside a hex digest; `quantum` repeats to pin
/// the per-term digest.
const DOC: &str = "zwibble quantum fox deploy retry quantum";

/// A common-word list of the size a dictionary attack starts from. It covers
/// four words the fixture actually stores, so the attack is not only against
/// terms the store never held.
const COMMON_WORDS: &[&str] = &[
    "the", "of", "and", "to", "in", "is", "that", "for", "it", "with", "as", "was", "on", "be",
    "at", "by", "this", "have", "from", "or", "one", "had", "not", "but", "what", "all", "were",
    "when", "we", "there", "can", "an", "your", "which", "their", "said", "if", "do", "will",
    "each", "about", "how", "up", "out", "them", "then", "she", "many", "some", "so", "quantum",
    "fox", "deploy", "retry",
];

/// One semantic fact with no readable needle anywhere; only the lexical path
/// below carries the fixture document.
fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.5),
        source_ref: Some("turn_1".to_owned()),
        context: None,
        snapshot: None,
        gist: Some("benign gist".to_owned()),
        valid_from: "t0".to_owned(),
        valid_until: None,
        tx_from: "t0".to_owned(),
        tx_until: None,
        created_at: "t0".to_owned(),
        updated_at: "t0".to_owned(),
    }
}

/// An independent key store: a second call produces different key material.
fn kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>
}

/// A scratch directory for one test, removed on drop including on panic.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(unique(tag));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The keyed column of one SQLite FTS row, read with raw SQL.
fn stored_content_text(store: &SqliteStore, bank: &str, id: &str) -> String {
    store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT content_text FROM items_fts WHERE item_id = ?1 AND bank_id = ?2",
                rusqlite::params![id, bank],
                |row| row.get(0),
            )
            .am("lexical content read")
        })
        .expect("read keyed column")
}

/// Existing files SQLite may keep the database in: the main file first, then
/// any journal sibling. Only paths that exist are returned.
fn store_files(path: &Path) -> Vec<PathBuf> {
    ["", "-wal", "-shm", "-journal"]
        .into_iter()
        .map(|suffix| {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            PathBuf::from(name)
        })
        .filter(|candidate| candidate.exists())
        .collect()
}

/// Does any window of `haystack` equal `needle`?
fn contains_bytes(haystack: &[u8], needle: &str) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|w| w == needle.as_bytes())
}

/// Lowercase hex of `bytes`.
fn hex_of(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Bare (unkeyed) SHA-256 hex: the digest a dictionary attacker can compute.
fn bare_sha256_hex(bytes: &[u8]) -> String {
    hex_of(&Sha256::digest(bytes))
}

#[test]
fn sqlite_lexical_index_carries_no_readable_word_and_leaves_no_file_trace() {
    let scratch = ScratchDir::new("lexical-security-file");
    let path = scratch.path().join("store.db");
    let bank = unique("bank");
    let id = unique("item");
    {
        let store = SqliteStore::open(&path, kms()).expect("open file-backed store");
        store
            .create_memory_item(&item(&id, &bank), "actor")
            .expect("seed item");
        store
            .put_lexical_doc(&bank, &id, DOC)
            .expect("key the document");

        let content = stored_content_text(&store, &bank, &id);
        assert!(
            !content.is_empty(),
            "the keyed column must hold the document's terms"
        );
        assert!(
            content.bytes().all(|b| b.is_ascii_digit()
                || (b'a'..=b'f').contains(&b)
                || b == b'k'
                || b == b' '),
            "stored column must be prefixed hex digests only: {content:?}"
        );
        for digest in content.split_whitespace() {
            assert_eq!(
                digest.len(),
                crate::lexical_digest::DIGEST_TERM_LEN,
                "one truncated HMAC digest per term: {digest:?}"
            );
        }
        for word in DOC.to_lowercase().split_whitespace() {
            assert!(
                !content.contains(word),
                "word `{word}` is readable in {content:?}"
            );
        }
    }

    // The handle is closed, so SQLite has flushed every page and journal.
    let files = store_files(&path);
    assert!(
        files.contains(&path),
        "the main store file must be one of the scanned files: {files:?}"
    );
    let mut saw_item_id = false;
    let mut saw_needle = false;
    for file in &files {
        let bytes = std::fs::read(file).expect("read store file");
        saw_item_id |= contains_bytes(&bytes, &id);
        saw_needle |= contains_bytes(&bytes, NEEDLE);
    }
    assert!(
        saw_item_id,
        "the scan must see bytes the store really wrote: {files:?}"
    );
    assert!(!saw_needle, "the needle reached disk in {files:?}");
}

#[test]
fn keyed_digests_differ_between_key_stores_and_repeat_within_one() {
    let bank = unique("bank");
    let first = SqliteStore::open_in_memory(kms()).expect("first store");
    let second = SqliteStore::open_in_memory(kms()).expect("second store");
    let first_id = unique("item");
    let second_id = unique("item");
    first
        .create_memory_item(&item(&first_id, &bank), "actor")
        .expect("seed first");
    second
        .create_memory_item(&item(&second_id, &bank), "actor")
        .expect("seed second");
    first
        .put_lexical_doc(&bank, &first_id, DOC)
        .expect("key in first");
    second
        .put_lexical_doc(&bank, &second_id, DOC)
        .expect("key in second");

    let under_first = stored_content_text(&first, &bank, &first_id);
    let under_second = stored_content_text(&second, &bank, &second_id);
    assert_ne!(
        under_first, under_second,
        "different key stores must key the same text differently"
    );

    // Same key store, same text: the stored form repeats exactly.
    let repeat_id = unique("item");
    first
        .create_memory_item(&item(&repeat_id, &bank), "actor")
        .expect("seed repeat");
    first
        .put_lexical_doc(&bank, &repeat_id, DOC)
        .expect("re-key");
    assert_eq!(stored_content_text(&first, &bank, &repeat_id), under_first);

    // A repeated term inside one document pins the per-term digest.
    let digests: Vec<&str> = under_first.split_whitespace().collect();
    assert_eq!(digests.len(), 6, "one digest per token: {digests:?}");
    assert_eq!(
        digests[1], digests[5],
        "the repeated term must key identically"
    );
}

#[test]
fn bare_hash_dictionary_recovers_no_stored_digest() {
    let bank = unique("bank");
    let id = unique("item");
    let store = SqliteStore::open_in_memory(kms()).expect("open store");
    store
        .create_memory_item(&item(&id, &bank), "actor")
        .expect("seed item");
    store
        .put_lexical_doc(&bank, &id, DOC)
        .expect("key the document");

    let content = stored_content_text(&store, &bank, &id);
    let digests: Vec<&str> = content.split_whitespace().collect();
    assert!(!digests.is_empty(), "no digests to attack");

    // The stored form is exactly the keyed encoding, so the only unknown an
    // attacker faces is the key.
    let key = store.kms().blind_index_key().expect("blind index key");
    assert_eq!(content, crate::lexical_digest::keyed_terms(&key, DOC));

    // The attacked dictionary: a real common-word list plus every word the
    // fixture actually stored.
    let mut dictionary: Vec<String> = COMMON_WORDS.iter().map(|w| (*w).to_owned()).collect();
    dictionary.extend(DOC.split_whitespace().map(|w| w.to_lowercase()));

    for word in &dictionary {
        let bare = bare_sha256_hex(word.as_bytes());
        // A fixed-salt unkeyed hash is still reversible by a dictionary.
        let fixed_salt = crate::erase_hash::sha256_hex(b"", word);
        for digest in &digests {
            // Compare the digest body, with the letter prefix removed: a
            // 33-char term never equals a 32- or 64-char hash, so comparing
            // whole strings would pass vacuously.
            let body = digest
                .strip_prefix('k')
                .expect("every stored term carries the prefix");
            assert_ne!(body, word.as_str(), "`{word}` was stored verbatim");
            assert_ne!(
                body,
                hex_of(word.as_bytes()),
                "`{word}` was stored hex-encoded"
            );
            assert_ne!(
                body, bare,
                "bare SHA-256 of `{word}` matched the stored digest"
            );
            assert_ne!(
                body,
                &bare[..crate::lexical_digest::DIGEST_HEX_LEN],
                "stored digest is a prefix of the bare SHA-256 of `{word}`"
            );
            assert_ne!(
                body, fixed_salt,
                "fixed-salt SHA-256 of `{word}` matched the stored digest"
            );
            assert_ne!(
                body,
                &fixed_salt[..crate::lexical_digest::DIGEST_HEX_LEN],
                "stored digest is a prefix of the fixed-salt SHA-256 of `{word}`"
            );
        }
    }
}

#[test]
fn keyed_lexical_search_still_matches_and_misses() {
    let bank = unique("bank");
    let id = unique("item");
    let store = SqliteStore::open_in_memory(kms()).expect("open store");
    store
        .create_memory_item(&item(&id, &bank), "actor")
        .expect("seed item");
    store
        .put_lexical_doc(&bank, &id, "deploy retry")
        .expect("key the document");

    let hits = store
        .lexical_search(&bank, "retry", &[], 5)
        .expect("search present word");
    assert_eq!(hits.len(), 1, "one open item holds the word: {hits:?}");
    assert_eq!(hits[0].item_id, id);

    // The query side keys the same way, so spelling does not change the match.
    let upper = store
        .lexical_search(&bank, "RETRY", &[], 5)
        .expect("search uppercase");
    assert_eq!(upper.len(), 1);
    assert_eq!(upper[0].item_id, id);

    let missing = store
        .lexical_search(&bank, "zephyrwombat", &[], 5)
        .expect("search absent word");
    assert!(
        missing.is_empty(),
        "an absent word must return no hits: {missing:?}"
    );
}

#[test]
fn postgres_lexical_column_carries_no_readable_word_and_search_round_trips() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = PostgresStore::open(&url, kms()).expect("open postgres store");
    let bank = unique("bank");
    let id = unique("item");
    store
        .create_memory_item(&item(&id, &bank), "actor")
        .expect("seed item");
    store
        .put_lexical_doc(&bank, &id, DOC)
        .expect("key the document");

    let tsv: String = store
        .with_client(|client| {
            let row = client
                .query_one(
                    "SELECT content_tsv::text FROM items WHERE id = $1 AND bank_id = $2",
                    &[&id, &bank],
                )
                .am("read content_tsv")?;
            Ok(row.get(0))
        })
        .expect("read tsvector text");
    assert!(
        !tsv.is_empty(),
        "the keyed tsvector must hold the document's terms"
    );
    assert!(
        !tsv.contains(NEEDLE),
        "needle readable in tsvector: {tsv:?}"
    );
    for word in DOC.to_lowercase().split_whitespace() {
        assert!(
            !tsv.to_lowercase().contains(word),
            "word `{word}` readable in tsvector: {tsv:?}"
        );
    }

    let hits = store
        .lexical_search(&bank, "retry", &[], 5)
        .expect("search the item");
    assert!(
        hits.iter().any(|hit| hit.item_id == id),
        "keyed search must still find the item: {hits:?}"
    );
}
