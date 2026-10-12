// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres tsvector/queue side of `IndexStore`.
//!
//! ## Owns
//! - Postgres `items.content_tsv` (tsvector + GIN) and `index_pending` SQL
//! - Postgres lexical search (`websearch_to_tsquery` + `ts_rank_cd`) and
//!   domain-filtered dense search with widening fetch
//!
//! ## Does not own
//! - The index contract definition (`index_store`)
//! - Embedding generation or worker scheduling (see the `clio-index` crate)
//!
//! ## Boundary
//!
//! Changes to Postgres index SQL should normally require changes here.
//! Changes to the backend-agnostic contract should not require changes here.
//!
//! ## Lexical honesty note
//!
//! Postgres native FTS (`ts_rank_cd`) is NOT classic BM25. This primitive
//! returns `ts_rank_cd`-shaped scores (higher = better); downstream hybrid
//! fusion must combine the two legs by rank (e.g. RRF), never by raw score
//! mixing across dense cosine and lexical ranks.
//!
//! Keep this module focused on Postgres index SQL.

use crate::index_store::{
    IndexStore, domain_predicate, encode_lexical_document, encode_lexical_query, require_bank,
};
use crate::migrate::{IntoAmError, now_iso};
use crate::model::{IndexCoverage, IndexJob, KnnHit, SearchHit};
use crate::postgres::PostgresStore;
use clio_types::{AmError, ErrorCode, SearchDomain};
use pgvector::Vector;

impl PostgresStore {
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
        self.with_client(|client| {
            let n = client
                .execute(
                    "INSERT INTO index_pending (bank_id, item_id, queued_at, updated_at)
                     VALUES ($1, $2, $3, $3)
                     ON CONFLICT (bank_id, item_id) DO NOTHING",
                    &[&bank_id, &item_id, &ts],
                )
                .am("index enqueue")?;
            Ok(n > 0)
        })
    }
}

/// In-transaction enqueue for the item create/update/triple paths: the job row
/// commits atomically with the leaf, so index work can never be silently lost.
pub(crate) fn enqueue_index_pending_tx(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<(), AmError> {
    let ts = now_iso();
    tx.execute(
        "INSERT INTO index_pending (bank_id, item_id, queued_at, updated_at)
         VALUES ($1, $2, $3, $3)
         ON CONFLICT (bank_id, item_id) DO NOTHING",
        &[&bank_id, &item_id, &ts],
    )
    .am("index enqueue")?;
    Ok(())
}

/// Current-belief visibility shared by both search legs and the raw KNN:
/// not discarded, the current transaction belief (`tx_until IS NULL`), not
/// archived.
pub(crate) const VISIBLE: &str =
    "items.discarded_at IS NULL AND items.tx_until IS NULL AND items.archived_at IS NULL";

impl IndexStore for PostgresStore {
    fn put_lexical_doc(&self, bank_id: &str, item_id: &str, text: &str) -> Result<(), AmError> {
        // The assembled text is keyed into digests here and nowhere else: the
        // plaintext never reaches `content_tsv`.
        let encoded = encode_lexical_document(self.kms().as_ref(), text)?;
        let n = self.with_client(|client| {
            client
                .execute(
                    "UPDATE items SET content_tsv = to_tsvector('simple', $3), updated_at = $4
                     WHERE id = $1 AND bank_id = $2",
                    &[&item_id, &bank_id, &encoded, &now_iso()],
                )
                .am("lexical put")
        })?;
        if n == 0 {
            return Err(AmError::new(
                ErrorCode::ItemNotFound,
                format!("item `{item_id}` not found in bank `{bank_id}`"),
            ));
        }
        Ok(())
    }

    fn delete_lexical_doc(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        let n = self.with_client(|client| {
            client
                .execute(
                    "UPDATE items SET content_tsv = NULL
                     WHERE id = $1 AND bank_id = $2 AND content_tsv IS NOT NULL",
                    &[&item_id, &bank_id],
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
        let has_rows: bool = self.with_client(|client| {
            let row = client
                .query_one(
                    "SELECT EXISTS(
                         SELECT 1 FROM items WHERE bank_id = $1 AND content_tsv IS NOT NULL
                     ) AS has_rows",
                    &[&bank_id],
                )
                .am("lexical index probe")?;
            Ok(row.get::<_, bool>(0))
        })?;
        if !has_rows {
            return Ok(vec![]);
        }
        let encoded_query = encode_lexical_query(self.kms().as_ref(), query)?;
        if encoded_query.is_empty() {
            return Ok(vec![]);
        }
        let dom = domain_predicate(domains);
        let sql = format!(
            "WITH q AS (SELECT websearch_to_tsquery('simple', $2) AS tsq)
             SELECT items.id, ts_rank_cd(items.content_tsv, q.tsq) AS score
             FROM items, q
             WHERE items.bank_id = $1
               AND items.content_tsv @@ q.tsq
               AND items.discarded_at IS NULL AND items.tx_until IS NULL
               AND items.archived_at IS NULL
               AND {dom}
             ORDER BY score DESC
             LIMIT $3"
        );
        let mut fetch = limit.max(1);
        loop {
            let rows = self.with_client(|client| {
                let stmt = client.prepare(&sql).am("lexical prepare")?;
                let mapped = client
                    .query(&stmt, &[&bank_id, &encoded_query, &(fetch as i64)])
                    .am("lexical query")?;
                let mut rows = Vec::new();
                for row in mapped {
                    rows.push((row.get::<_, String>(0), f64::from(row.get::<_, f32>(1))));
                }
                Ok(rows)
            })?;
            let fetched = rows.len();
            let mut out = Vec::new();
            for (item_id, score) in rows {
                out.push(SearchHit { item_id, score });
                if out.len() >= limit {
                    return Ok(out);
                }
            }
            if fetched < fetch {
                return Ok(out);
            }
            fetch = fetch.checked_mul(2).expect("lexical fetch overflow");
        }
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
        let dom = domain_predicate(domains);
        // Repeat the halfvec cast in ORDER BY for widths above the plain vector
        // index limit, or the expression index is not used (sequential scan).
        let distance = crate::postgres::distance_expr(self.space(), "e");
        let sql = format!(
            "SELECT e.item_id, ({distance}) AS distance
             FROM item_embeddings e
             JOIN items ON items.id = e.item_id AND items.bank_id = e.bank_id
             WHERE e.bank_id = $2
               AND {VISIBLE}
               AND {dom}
             ORDER BY {distance}
             LIMIT $3"
        );
        let embedding = Vector::from(query.to_vec());
        self.validate_dims(query)?;
        let mut fetch = limit.max(1);
        loop {
            let rows = self.with_client(|client| {
                let stmt = client.prepare(&sql).am("dense prepare")?;
                let mapped = client
                    .query(&stmt, &[&embedding, &bank_id, &(fetch as i64)])
                    .am("dense query")?;
                let mut rows = Vec::new();
                for row in mapped {
                    rows.push(KnnHit {
                        item_id: row.get(0),
                        distance: row.get(1),
                    });
                }
                Ok(rows)
            })?;
            let fetched = rows.len();
            let mut out = Vec::new();
            for hit in rows {
                out.push(hit);
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
        self.with_client(|client| {
            let rows = client
                .query(
                    "SELECT bank_id, item_id, attempts, last_error
                     FROM index_pending
                     WHERE ($1::text IS NULL OR last_attempt_at IS NULL OR last_attempt_at < $1)
                       AND attempts < $2
                     ORDER BY queued_at
                     LIMIT $3",
                    &[
                        &backoff_cutoff_ms,
                        &i32::try_from(max_attempts).unwrap_or(i32::MAX),
                        &(limit as i64),
                    ],
                )
                .am("pending rows")?;
            Ok(rows
                .iter()
                .map(|r| IndexJob {
                    bank_id: r.get(0),
                    item_id: r.get(1),
                    attempts: u32::try_from(r.get::<_, i32>(2).max(0)).unwrap_or(0),
                    last_error: r.get(3),
                })
                .collect())
        })
    }

    fn pending_index_job(
        &self,
        bank_id: &str,
        item_id: &str,
        backoff_cutoff_ms: Option<&str>,
        max_attempts: u32,
    ) -> Result<Option<IndexJob>, AmError> {
        self.with_client(|client| {
            let row = client
                .query_opt(
                    "SELECT bank_id, item_id, attempts, last_error
                     FROM index_pending
                     WHERE bank_id = $1 AND item_id = $2
                       AND ($3::text IS NULL OR last_attempt_at IS NULL OR last_attempt_at < $3)
                       AND attempts < $4",
                    &[
                        &bank_id,
                        &item_id,
                        &backoff_cutoff_ms,
                        &i32::try_from(max_attempts).unwrap_or(i32::MAX),
                    ],
                )
                .am("pending job row")?;
            Ok(row.map(|r| IndexJob {
                bank_id: r.get(0),
                item_id: r.get(1),
                attempts: u32::try_from(r.get::<_, i32>(2).max(0)).unwrap_or(0),
                last_error: r.get(3),
            }))
        })
    }

    fn finish_index_job(&self, bank_id: &str, item_id: &str) -> Result<(), AmError> {
        self.with_client(|client| {
            client
                .execute(
                    "DELETE FROM index_pending WHERE bank_id = $1 AND item_id = $2",
                    &[&bank_id, &item_id],
                )
                .am("index finish")?;
            Ok(())
        })
    }

    fn fail_index_job(&self, bank_id: &str, item_id: &str, error: &str) -> Result<(), AmError> {
        let ts = now_iso();
        self.with_client(|client| {
            client
                .execute(
                    "UPDATE index_pending
                     SET attempts = attempts + 1, last_error = $3,
                         last_attempt_at = $4, updated_at = $4
                     WHERE bank_id = $1 AND item_id = $2",
                    &[&bank_id, &item_id, &error, &ts],
                )
                .am("index fail")?;
            Ok(())
        })
    }

    fn index_coverage(&self, bank_id: Option<&str>) -> Result<IndexCoverage, AmError> {
        self.with_client(|client| crate::postgres_coverage::index_coverage_pg(client, bank_id))
    }

    fn indexable_items(
        &self,
        bank_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<String>, AmError> {
        require_bank(bank_id)?;
        self.with_client(|client| {
            let rows = client
                .query(
                    "SELECT id FROM items
                     WHERE bank_id = $1 AND tx_until IS NULL AND discarded_at IS NULL
                     ORDER BY created_at, id LIMIT $2 OFFSET $3",
                    &[&bank_id, &(limit as i64), &(offset as i64)],
                )
                .am("indexable rows")?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        })
    }

    fn count_indexable_items(&self, bank_id: &str) -> Result<u64, AmError> {
        require_bank(bank_id)?;
        self.with_client(|client| {
            let n = client
                .query_one(
                    "SELECT COUNT(*) FROM items
                     WHERE bank_id = $1 AND tx_until IS NULL AND discarded_at IS NULL",
                    &[&bank_id],
                )
                .am("indexable count")?;
            let v: i64 = n.get(0);
            Ok(u64::try_from(v.max(0)).unwrap_or(0))
        })
    }

    fn indexable_banks(&self) -> Result<Vec<String>, AmError> {
        self.with_client(|client| {
            let rows = client
                .query(
                    "SELECT DISTINCT bank_id FROM items
                     WHERE tx_until IS NULL AND discarded_at IS NULL
                     ORDER BY bank_id",
                    &[],
                )
                .am("banks rows")?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        })
    }

    fn cleanup_orphans(&self, bank_id: Option<&str>) -> Result<u64, AmError> {
        // Vector rows cascade with the item (real FK) and content_tsv lives on
        // the item row; only outbox entries can outlive their item. A bank scope
        // filters by bank_id so a scoped reindex never touches other banks.
        self.with_client(|client| {
            let bank_filter = bank_id.map_or("", |_| " AND bank_id = $1");
            let sql = format!(
                "DELETE FROM index_pending WHERE item_id NOT IN (SELECT id FROM items){bank_filter}"
            );
            let n = match bank_id {
                Some(bank) => client
                    .execute(sql.as_str(), &[&bank])
                    .am("orphan cleanup")?,
                None => client.execute(sql.as_str(), &[]).am("orphan cleanup")?,
            };
            Ok(n)
        })
    }
}
