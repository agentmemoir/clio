// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite FTS5/queue side of `IndexStore`.
//!
//! ## Owns
//! - SQLite `items_fts` (FTS5) and `index_pending` SQL
//! - SQLite lexical search and domain-filtered dense search with widening fetch
//!
//! ## Does not own
//! - The index contract definition (`index_store`)
//! - Embedding generation or worker scheduling (see the `clio-index` crate)
//!
//! ## Boundary
//!
//! Changes to SQLite index SQL should normally require changes here.
//! Changes to the backend-agnostic contract should not require changes here.
//!
//! Keep this module focused on SQLite index SQL.

use crate::index_store::{
    IndexStore, domain_predicate, encode_lexical_document, encode_lexical_query, fts5_query,
};
use crate::migrate::{IntoAmError, now_iso};
use crate::model::{IndexCoverage, IndexJob, KnnHit, SearchHit};
use crate::sqlite::SqliteStore;
use crate::sqlite_knn::knn_in_bank;
use clio_types::{AmError, ErrorCode, SearchDomain};
use rusqlite::params;

impl SqliteStore {
    /// Durable idempotent enqueue used by the item write paths (outbox).
    pub(crate) fn enqueue_index_pending(
        &self,
        bank_id: &str,
        item_id: &str,
    ) -> Result<bool, AmError> {
        if bank_id.is_empty() || item_id.is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "bank_id and item_id are required to enqueue an index job",
            ));
        }
        let ts = now_iso();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO index_pending (bank_id, item_id, queued_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                params![bank_id, item_id, ts],
            )
            .am("index enqueue")?;
            Ok(conn.changes() > 0)
        })
    }
}

/// In-transaction enqueue for the item create/update/triple paths: the job row
/// commits atomically with the leaf, so index work can never be silently lost.
pub(crate) fn enqueue_index_pending_tx(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<(), AmError> {
    let ts = now_iso();
    tx.execute(
        "INSERT OR IGNORE INTO index_pending (bank_id, item_id, queued_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)",
        params![bank_id, item_id, ts],
    )
    .am("index enqueue")?;
    Ok(())
}

impl IndexStore for SqliteStore {
    fn put_lexical_doc(&self, bank_id: &str, item_id: &str, text: &str) -> Result<(), AmError> {
        // The assembled text is keyed into digests here and nowhere else: the
        // plaintext never reaches the FTS table.
        let encoded = encode_lexical_document(self.kms().as_ref(), text)?;
        self.with_conn(|conn| {
            let tx = conn.transaction().am("lexical upsert begin")?;
            let exists: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM items WHERE id = ?1 AND bank_id = ?2",
                    params![item_id, bank_id],
                    |r| r.get(0),
                )
                .am("lexical item lookup")?;
            if exists == 0 {
                return Err(AmError::new(
                    ErrorCode::ItemNotFound,
                    format!("item `{item_id}` not found in bank `{bank_id}`"),
                ));
            }
            tx.execute(
                "DELETE FROM items_fts WHERE item_id = ?1 AND bank_id = ?2",
                params![item_id, bank_id],
            )
            .am("lexical replace delete")?;
            tx.execute(
                "INSERT INTO items_fts (item_id, bank_id, content_text) VALUES (?1, ?2, ?3)",
                params![item_id, bank_id, encoded],
            )
            .am("lexical insert")?;
            tx.commit().am("lexical commit")?;
            Ok(())
        })
    }

    fn delete_lexical_doc(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        let n = self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM items_fts WHERE item_id = ?1 AND bank_id = ?2",
                params![item_id, bank_id],
            )
            .am("lexical delete")
        })?;
        Ok(n > 0)
    }

    fn lexical_search(
        &self,
        bank_id: &str,
        query: &str,
        domains: &[SearchDomain],
        limit: usize,
    ) -> Result<Vec<SearchHit>, AmError> {
        require_bank(bank_id)?;
        if query.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "lexical query must be non-empty",
            ));
        }
        if limit == 0 {
            return Ok(vec![]);
        }
        // An empty index needs no key: nothing can match, so answer the
        // honest "no results" without touching key material. When rows exist,
        // resolving the key is required, and a missing one fails loudly.
        let has_rows: bool = self.with_conn(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM items_fts WHERE bank_id = ?1)",
                params![bank_id],
                |row| row.get::<_, i64>(0),
            )
            .am("lexical index probe")
            .map(|n| n != 0)
        })?;
        if !has_rows {
            return Ok(vec![]);
        }
        let fts_query = fts5_query(&encode_lexical_query(self.kms().as_ref(), query)?);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let dom = domain_predicate(domains);
        // SQL-level domain/visibility filtering already applies the limit, so
        // this leg never needs the dense widening fetch.
        let sql = format!(
            "SELECT items_fts.item_id, -bm25(items_fts) AS score
             FROM items_fts
             JOIN items ON items.id = items_fts.item_id
                        AND items.bank_id = items_fts.bank_id
             WHERE items_fts MATCH ?1
               AND items_fts.bank_id = ?2
               AND items.discarded_at IS NULL AND items.tx_until IS NULL
               AND items.archived_at IS NULL
               AND {dom}
             ORDER BY rank
             LIMIT ?3"
        );
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&sql).am("lexical prepare")?;
            let mapped = stmt
                .query_map(params![fts_query, bank_id, limit as i64], |r| {
                    Ok(SearchHit {
                        item_id: r.get(0)?,
                        score: r.get(1)?,
                    })
                })
                .am("lexical query")?;
            let mut out = Vec::new();
            for row in mapped {
                out.push(row.am("lexical row")?);
            }
            Ok(out)
        })
    }

    fn dense_search(
        &self,
        bank_id: &str,
        query: &[f32],
        domains: &[SearchDomain],
        limit: usize,
    ) -> Result<Vec<KnnHit>, AmError> {
        require_bank(bank_id)?;
        if limit == 0 {
            return Ok(vec![]);
        }
        self.validate_dims(query)?;
        let dom = domain_predicate(domains);
        let mut fetch = limit.max(1);
        loop {
            let hits = self.with_conn(|conn| knn_in_bank(conn, bank_id, query, fetch))?;
            let fetched = hits.len();
            let mut out = Vec::new();
            for hit in hits {
                if self.sqlite_item_visible(&hit.item_id, bank_id, &dom)? {
                    out.push(hit);
                }
                if out.len() >= limit {
                    return Ok(out);
                }
            }
            if fetched < fetch {
                return Ok(out);
            }
            fetch = fetch.checked_mul(2).expect("dense fetch overflow");
        }
    }

    fn enqueue_index_job(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        self.enqueue_index_pending(bank_id, item_id)
    }

    fn pending_index_jobs(
        &self,
        limit: usize,
        backoff_cutoff_ms: Option<&str>,
        max_attempts: u32,
    ) -> Result<Vec<IndexJob>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT bank_id, item_id, attempts, last_error
                     FROM index_pending
                     WHERE (?1 IS NULL OR last_attempt_at IS NULL OR last_attempt_at < ?1)
                       AND attempts < ?2
                     ORDER BY queued_at
                     LIMIT ?3",
                )
                .am("pending prepare")?;
            let rows = stmt
                .query_map(
                    params![backoff_cutoff_ms, i64::from(max_attempts), limit as i64],
                    |r| {
                        Ok(IndexJob {
                            bank_id: r.get(0)?,
                            item_id: r.get(1)?,
                            attempts: u32::try_from(r.get::<_, i64>(2)?.max(0)).unwrap_or(0),
                            last_error: r.get(3)?,
                        })
                    },
                )
                .am("pending rows")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("pending row")?);
            }
            Ok(out)
        })
    }

    fn finish_index_job(&self, bank_id: &str, item_id: &str) -> Result<(), AmError> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM index_pending WHERE bank_id = ?1 AND item_id = ?2",
                params![bank_id, item_id],
            )
            .am("index finish")?;
            Ok(())
        })
    }

    fn fail_index_job(&self, bank_id: &str, item_id: &str, error: &str) -> Result<(), AmError> {
        let ts = now_iso();
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE index_pending
                 SET attempts = attempts + 1, last_error = ?3,
                     last_attempt_at = ?4, updated_at = ?4
                 WHERE bank_id = ?1 AND item_id = ?2",
                params![bank_id, item_id, error, ts],
            )
            .am("index fail")?;
            Ok(())
        })
    }

    fn index_coverage(&self, bank_id: Option<&str>) -> Result<IndexCoverage, AmError> {
        self.with_conn(|conn| crate::sqlite_coverage::index_coverage_sqlite(conn, bank_id))
    }

    fn indexable_items(
        &self,
        bank_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, AmError> {
        require_bank(bank_id)?;
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM items
                     WHERE bank_id = ?1 AND tx_until IS NULL AND discarded_at IS NULL
                     ORDER BY created_at, id LIMIT ?2 OFFSET ?3",
                )
                .am("indexable prepare")?;
            let rows = stmt
                .query_map(params![bank_id, limit as i64, offset as i64], |r| {
                    r.get::<_, String>(0)
                })
                .am("indexable rows")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("indexable row")?);
            }
            Ok(out)
        })
    }

    fn count_indexable_items(&self, bank_id: &str) -> Result<u64, AmError> {
        require_bank(bank_id)?;
        let n: i64 = self.with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM items
                 WHERE bank_id = ?1 AND tx_until IS NULL AND discarded_at IS NULL",
                params![bank_id],
                |r| r.get(0),
            )
            .am("indexable count")
        })?;
        Ok(u64::try_from(n.max(0)).unwrap_or(0))
    }

    fn indexable_banks(&self) -> Result<Vec<String>, AmError> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT bank_id FROM items
                     WHERE tx_until IS NULL AND discarded_at IS NULL
                     ORDER BY bank_id",
                )
                .am("banks prepare")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .am("banks rows")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("banks row")?);
            }
            Ok(out)
        })
    }

    fn cleanup_orphans(&self, bank_id: Option<&str>) -> Result<u64, AmError> {
        // vec0/FTS5 tables do not enforce FK cascade; sweep rows whose item is
        // gone. `NOT IN` over an empty items set removes every derived row,
        // which is correct (no items → all derived rows are orphans). A bank
        // scope filters every table by its `bank_id` column so a scoped
        // reindex never deletes derived rows of other banks.
        let bank_filter = if bank_id.is_some() {
            " AND bank_id = ?1"
        } else {
            ""
        };
        self.with_conn(|conn| {
            let mut removed = 0_u64;
            for table in ["item_embeddings", "items_fts", "index_pending"] {
                let sql = format!(
                    "DELETE FROM {table} WHERE item_id NOT IN (SELECT id FROM items){bank_filter}"
                );
                let n = match bank_id {
                    Some(bank) => conn.execute(&sql, params![bank]).am("orphan cleanup")?,
                    None => conn.execute(&sql, []).am("orphan cleanup")?,
                };
                removed += n as u64;
            }
            Ok(removed)
        })
    }
}

impl SqliteStore {
    /// Domain + current-belief visibility of one item row.
    fn sqlite_item_visible(
        &self,
        item_id: &str,
        bank_id: &str,
        dom: &str,
    ) -> Result<bool, AmError> {
        let sql = format!(
            "SELECT COUNT(*) FROM items
             WHERE id = ?1 AND bank_id = ?2
               AND items.discarded_at IS NULL AND items.tx_until IS NULL
               AND items.archived_at IS NULL
               AND {dom}"
        );
        let n: i64 = self.with_conn(|conn| {
            conn.query_row(&sql, params![item_id, bank_id], |r| r.get(0))
                .am("dense visibility")
        })?;
        Ok(n > 0)
    }
}

fn require_bank(bank_id: &str) -> Result<(), AmError> {
    if bank_id.is_empty() {
        return Err(AmError::new(ErrorCode::MissingBank, "bank is required"));
    }
    Ok(())
}
