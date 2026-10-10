// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the dense/lexical index-maintenance contract
//! shared by both persistence backends.
//!
//! ## Owns
//! - `IndexStore` behavioral contract (queue ops, lexical docs, scoped search, coverage)
//! - Shared search filter SQL fragment assembly
//!
//! ## Does not own
//! - Backend SQL statements (see `sqlite_index` / `postgres_index`)
//! - Embedding generation or the index worker (see the `clio-index` crate)
//!
//! ## Boundary
//!
//! Changes to the index contract surface should normally require changes here.
//! Changes to driver SQL or embedding should not require changes here.
//!
//! Keep this module focused on the backend-agnostic index contract.

use crate::model::{IndexCoverage, IndexJob, KnnHit, SearchHit};
use clio_types::{AmError, SearchDomain};
use std::fmt::Write as _;

/// Dual-backend index pipeline repository (queue + lexical + scoped search).
///
/// Search visibility policy (both legs, both backends): only rows whose item is
/// the current transaction belief (`tx_until IS NULL`) and not discarded
/// (`discarded_at IS NULL`) are searchable. Invalidated items keep their rows
/// (bi-temporal reads stay possible) but drop out of current-belief search.
/// Lexical text and vectors are derived state: safe to wipe and rebuild.
pub trait IndexStore: Send + Sync {
    /// Upsert the lexical document for one item (FTS5 row / `content_tsv`).
    ///
    /// `text` is the assembled plaintext; the implementation keys it into
    /// per-term digests under the store's blind-index key before anything is
    /// stored, so the persisted column holds no readable word. Callers pass the
    /// same text they always did: keying is a property of the stored form, not
    /// of the call.
    ///
    /// # Errors
    /// Missing item or storage failures.
    fn put_lexical_doc(&self, bank_id: &str, item_id: &str, text: &str) -> Result<(), AmError>;

    /// Remove the lexical document for one item.
    ///
    /// # Errors
    /// Storage failures.
    fn delete_lexical_doc(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError>;

    /// BM25-style lexical search within one bank.
    ///
    /// Ranks best-first with a normalized score (higher = better). The query is
    /// expanded into the same keyed digests the stored form carries before it
    /// reaches SQL, so an indexed word and its query spelling meet, and neither
    /// side exposes the word. Domain and current-belief filters are applied
    /// in-query; the fetch widens until the limit is filled or candidates are
    /// exhausted.
    ///
    /// An index with no rows for the bank answers with an empty result without
    /// resolving the blind-index key: nothing can match, so no key is needed.
    /// A non-empty index whose key store cannot produce the key fails loudly
    /// instead of returning an empty page.
    ///
    /// # Errors
    /// Empty bank, empty query, a missing blind-index key on a non-empty
    /// index, or storage failures.
    fn lexical_search(
        &self,
        bank_id: &str,
        query: &str,
        domains: &[SearchDomain],
        limit: usize,
    ) -> Result<Vec<SearchHit>, AmError>;

    /// Cosine KNN within one bank, filtered in-query by domains.
    ///
    /// Returns hits ordered nearest-first (`KnnHit.distance`, lower = better).
    ///
    /// # Errors
    /// Empty bank, dimension mismatch, or storage failures.
    fn dense_search(
        &self,
        bank_id: &str,
        query: &[f32],
        domains: &[SearchDomain],
        limit: usize,
    ) -> Result<Vec<KnnHit>, AmError>;

    /// Durable, idempotent enqueue of one index job (transactional outbox row).
    /// Returns `true` when the row was newly queued.
    ///
    /// # Errors
    /// Empty bank/id or storage failures.
    fn enqueue_index_job(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError>;

    /// Due pending jobs (oldest first).
    ///
    /// `backoff_cutoff_ms` (epoch-millis text, matching `last_attempt_at`) skips
    /// rows retried too recently; pass `None` to ignore the retry window (forced
    /// retry). Rows whose `attempts` reached `max_attempts` are never returned:
    /// the worker does not retry them, so they must not hold batch slots ahead of
    /// newer jobs. Pass `u32::MAX` to read every pending row regardless (tests,
    /// backlog reporting).
    ///
    /// # Errors
    /// Storage failures.
    fn pending_index_jobs(
        &self,
        limit: usize,
        backoff_cutoff_ms: Option<&str>,
        max_attempts: u32,
    ) -> Result<Vec<IndexJob>, AmError>;

    /// Remove a completed (or skipped) queue row.
    ///
    /// # Errors
    /// Storage failures.
    fn finish_index_job(&self, bank_id: &str, item_id: &str) -> Result<(), AmError>;

    /// Record one failed attempt (increments `attempts`, stamps error + time).
    ///
    /// # Errors
    /// Storage failures.
    fn fail_index_job(&self, bank_id: &str, item_id: &str, error: &str) -> Result<(), AmError>;

    /// Index pipeline coverage (counts only; PII-safe).
    ///
    /// # Errors
    /// Storage failures.
    fn index_coverage(&self, bank_id: Option<&str>) -> Result<IndexCoverage, AmError>;

    /// Open, non-discarded item ids that carry indexable dual content, oldest
    /// first (rebuild sweep order, offset-paginated).
    ///
    /// # Errors
    /// Empty bank or storage failures.
    fn indexable_items(
        &self,
        bank_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, AmError>;

    /// Count open, non-discarded items that carry indexable dual content.
    ///
    /// # Errors
    /// Empty bank or storage failures.
    fn count_indexable_items(&self, bank_id: &str) -> Result<u64, AmError>;

    /// Banks that currently hold indexable items (rebuild scopes).
    ///
    /// # Errors
    /// Storage failures.
    fn indexable_banks(&self) -> Result<Vec<String>, AmError>;

    /// Remove derived rows (vectors, lexical docs, queue entries) whose item
    /// row vanished. Returns the number of rows removed.
    ///
    /// When `bank_id` is `Some`, only rows belonging to that bank are touched:
    /// a bank-scoped reindex must never delete derived rows of other banks.
    /// `None` sweeps the whole store.
    ///
    /// # Errors
    /// Storage failures.
    fn cleanup_orphans(&self, bank_id: Option<&str>) -> Result<u64, AmError>;
}

/// Encode one assembled item text into the keyed term digests the index stores.
///
/// Both backends call this from `put_lexical_doc`, so the stored form is
/// produced in exactly one place. The key never leaves the key store as text.
///
/// # Errors
/// The blind-index key cannot be resolved.
pub(crate) fn encode_lexical_document(
    kms: &dyn crate::dek::DekProvider,
    text: &str,
) -> Result<String, AmError> {
    Ok(crate::lexical_digest::keyed_terms(
        // Write path: creating the key here is expected, like any DEK ensure.
        &kms.ensure_blind_index_key()?,
        text,
    ))
}

/// Expand one query into the keyed term digests the index stores.
///
/// This is the query half of the same transformation
/// [`encode_lexical_document`] applies at index time: an inflected or
/// differently-cased query term produces the digest of the stored base form.
///
/// The key is resolved with the read-path lookup, never the creating ensure:
/// a search must not be able to create key material. When the key store is
/// absent or holds no blind-index slot, the error propagates to the caller
/// instead of degrading into an empty result set.
///
/// # Errors
/// The blind-index key cannot be resolved without creating it.
pub(crate) fn encode_lexical_query(
    kms: &dyn crate::dek::DekProvider,
    query: &str,
) -> Result<String, AmError> {
    Ok(crate::lexical_digest::keyed_terms(
        &kms.blind_index_key()?,
        query,
    ))
}

/// Build the shared `kind/category/episodic_type` SQL fragment for domains.
///
/// Returns an always-true fragment when `domains` is empty. Both drivers share
/// identical text quoting for these literal columns.
#[must_use]
pub(crate) fn domain_predicate(domains: &[SearchDomain]) -> String {
    if domains.is_empty() {
        return "1 = 1".to_owned();
    }
    let parts: Vec<String> = domains
        .iter()
        .map(|d| {
            let (kind, category, episodic_type) = d.item_predicates();
            let mut frag = format!("(items.kind = '{kind}'");
            if let Some(cat) = category {
                let _ = write!(frag, " AND items.category = '{cat}'");
            }
            if let Some(ety) = episodic_type {
                let _ = write!(frag, " AND items.episodic_type = '{ety}'");
            }
            frag.push(')');
            frag
        })
        .collect();
    format!("({})", parts.join(" OR "))
}

/// Escape FTS5 query syntax: double embedded quotes and quote each token so
/// user text is treated as terms, not FTS5 operators.
///
/// Tokens are joined with `OR` (any-term match, BM25 ranks the rest). An
/// all-punctuation input yields an empty result rather than syntax errors.
#[must_use]
pub fn fts5_query(query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    terms.join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_predicate_shapes() {
        assert_eq!(domain_predicate(&[]), "1 = 1");
        let task = SearchDomain::parse("task").expect("task");
        assert_eq!(
            domain_predicate(&[task]),
            "((items.kind = 'episodic' AND items.episodic_type = 'task'))"
        );
        let multi = domain_predicate(&[SearchDomain::Persona, SearchDomain::Failure]);
        assert!(multi.contains("items.category = 'persona'"));
        assert!(multi.contains("items.episodic_type = 'failure'"));
        assert!(multi.starts_with("(("));
        assert!(multi.contains(" OR "));
    }

    #[test]
    fn fts5_query_escapes_and_joins() {
        assert_eq!(
            fts5_query("ECONNRESET deploy"),
            "\"ECONNRESET\" OR \"deploy\""
        );
        assert_eq!(fts5_query("say \"hi\""), "\"say\" OR \"\"\"hi\"\"\"");
        assert_eq!(fts5_query(""), "");
        // FTS5 syntax characters are inert inside quoted terms.
        assert_eq!(fts5_query("a* (b)"), "\"a*\" OR \"(b)\"");
    }
}

#[cfg(test)]
#[path = "index_store_tests.rs"]
mod index_store_tests;

#[cfg(test)]
#[path = "index_store_discard_tests.rs"]
mod index_store_discard_tests;

#[cfg(test)]
#[path = "index_store_lexical_migration_tests.rs"]
mod index_store_lexical_migration_tests;

#[cfg(test)]
#[path = "index_store_lexical_query_key_tests.rs"]
mod index_store_lexical_query_key_tests;

#[cfg(test)]
#[path = "index_store_lexical_security_tests.rs"]
mod index_store_lexical_security_tests;
