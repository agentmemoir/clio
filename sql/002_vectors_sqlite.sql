-- SQLite dialect for dense vectors via sqlite-vec (exact / brute-force KNN).
-- Formatted per .agents/skills/sql-guidelines where SQLite allows.
-- Apply after 001_core.sql. The sqlite-vec extension MUST be loaded first;
-- silent float-blob fallback is forbidden.
--
-- Width-independent DDL only: the `item_embeddings` vec0 table is generated for
-- the resolved embedding space width at bootstrap (see clio-store::vector_ddl).
--
-- Ceiling: vec0 exact KNN is comfortable for tens–low hundreds of thousands of
-- vectors; ANN is out of scope for this schema revision.

PRAGMA foreign_keys = ON;

-- Lexical retrieval (keyed blind index). content_text holds one keyed term
-- digest per stemmed token of the item's assembled text: space-joined lowercase
-- `k`-prefixed 32-hex-char HMAC-SHA256 truncations, written and replaced by the
-- index path (`clio ops reindex --target lexical` rewrites pre-existing rows).
-- No readable word is stored. Tokenisation is applied before hashing and is the
-- same on the write path and the query path (lowercase, accent folding, Porter
-- stemming); the FTS5 porter tokenizer then applies to the hex digest tokens,
-- identically on both sides.
CREATE VIRTUAL TABLE IF NOT EXISTS items_fts
USING fts5
(
  item_id UNINDEXED,
  bank_id UNINDEXED,
  content_text,
  tokenize = 'porter'
);

-- Correction lineage columns. 001_core.sql declares them for
-- fresh databases; the SQLite convergence hook adds them to pre-existing
-- databases before this file runs. Indexes are created here so they never
-- reference a column a legacy database has not yet been given.
CREATE INDEX IF NOT EXISTS items_superseded_by_id_inx
ON items (superseded_by_id);

CREATE INDEX IF NOT EXISTS items_corrects_item_id_inx
ON items (corrects_item_id);

-- ---------------------------------------------------------------------------
-- audit_events immutability: append-only telemetry.
-- UPDATE and DELETE must fail at the database level; history is never altered
-- outside verified compliance erasure (a later phase).
-- ---------------------------------------------------------------------------
CREATE TRIGGER IF NOT EXISTS audit_events_no_update
BEFORE UPDATE ON audit_events
BEGIN
  SELECT RAISE(ABORT, 'audit_events is append-only: UPDATE rejected');
END;

CREATE TRIGGER IF NOT EXISTS audit_events_no_delete
BEFORE DELETE ON audit_events
BEGIN
  SELECT RAISE(ABORT, 'audit_events is append-only: DELETE rejected');
END;
