-- Postgres / pgvector dialect (Compose default).
-- Formatted per .agents/skills/sql-guidelines.
-- Applied after 001_core.sql. Width-independent DDL only: `item_embeddings` and
-- its index are generated for the resolved embedding space width at bootstrap
-- (see clio-store::vector_ddl), so one schema serves any configured model width.

CREATE EXTENSION IF NOT EXISTS vector;

-- Optional lexical aid (Postgres). SQLite uses FTS5 in 002_vectors_sqlite.sql.
-- content_tsv holds `to_tsvector('simple', ...)` over keyed term digests:
-- space-joined lowercase `k`-prefixed 32-hex-char HMAC-SHA256 truncations, one
-- per stemmed token of the item's assembled text, so the column carries no
-- readable word. Tokenisation
-- before hashing is the same on the write path and the query path (lowercase,
-- accent folding, Porter stemming); `simple` then tokenises the hex digests
-- identically on both sides. `clio ops reindex --target lexical` rewrites rows
-- written before this form existed.
ALTER TABLE items ADD COLUMN IF NOT EXISTS content_tsv tsvector;

CREATE INDEX IF NOT EXISTS items_content_tsv_inx
ON items
USING gin (content_tsv);

-- Correction lineage columns. 001_core.sql declares them for
-- fresh databases; pre-existing Postgres databases converge here.
ALTER TABLE items ADD COLUMN IF NOT EXISTS corrects_item_id text;
ALTER TABLE items ADD COLUMN IF NOT EXISTS superseded_by_id text;

-- Optional bounded descriptive source `context` carriers. 001_core.sql
-- declares them for fresh databases; pre-existing Postgres databases converge
-- here. Sealed ciphertext columns only: no plaintext context at rest.
ALTER TABLE beliefs ADD COLUMN IF NOT EXISTS context_cipher text;
ALTER TABLE persona_stable_entries ADD COLUMN IF NOT EXISTS context_cipher text;
ALTER TABLE persona_preferences ADD COLUMN IF NOT EXISTS context_cipher text;
-- Effective time of the currently stored sealed context. 001_core.sql
-- declares them for fresh databases; pre-existing Postgres databases converge here.
ALTER TABLE persona_preferences ADD COLUMN IF NOT EXISTS context_as_of text;
ALTER TABLE beliefs ADD COLUMN IF NOT EXISTS context_as_of text;

-- Sealed triple VALUE carrier. 001_core.sql declares it for
-- fresh databases; pre-existing Postgres databases converge here. A table
-- that still carries the legacy plaintext `object` column stays refused on
-- open; restore a converted backup, then retry.
ALTER TABLE triples ADD COLUMN IF NOT EXISTS object_ciphertext text;

-- Distilled-unit reconciliation state. 001_core.sql declares these for fresh
-- databases; pre-existing Postgres databases converge here. The constant
-- integer default is metadata-only on ALTER (no table rewrite); pre-v2 rows
-- read revision 1 and no stale state.
ALTER TABLE distilled_units ADD COLUMN IF NOT EXISTS revision integer NOT NULL DEFAULT 1;
ALTER TABLE distilled_units ADD COLUMN IF NOT EXISTS stale_reason text;
ALTER TABLE distilled_units ADD COLUMN IF NOT EXISTS stale_detail text;
ALTER TABLE distilled_units ADD COLUMN IF NOT EXISTS stale_at text;
-- Per-revision evidence trail (record shape v3). A constant default, which is
-- what `ADD COLUMN` accepts on a populated table; pre-v3 rows read an empty
-- trail and the next revision a wave writes starts one.
ALTER TABLE distilled_units ADD COLUMN IF NOT EXISTS evidence_json text NOT NULL DEFAULT '[]';

-- Persona-context erasure tombstone. 001_core.sql declares it
-- for fresh databases; the IF NOT EXISTS form converges pre-existing ones.
CREATE TABLE IF NOT EXISTS persona_erase_tombstones
(
  bank_id                                                                                      text PRIMARY KEY,
  erased_at                                                                                    text NOT NULL
);

CREATE INDEX IF NOT EXISTS items_superseded_by_id_inx
ON items (superseded_by_id);

CREATE INDEX IF NOT EXISTS items_corrects_item_id_inx
ON items (corrects_item_id);

-- ---------------------------------------------------------------------------
-- audit_events immutability: append-only telemetry.
-- UPDATE and DELETE must fail at the database level; history is never altered
-- outside verified compliance erasure (a later phase).
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION audit_events_immutable()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  RAISE EXCEPTION 'audit_events is append-only: % rejected', TG_OP;
END;
$$;

DROP TRIGGER IF EXISTS audit_events_no_update ON audit_events;
CREATE TRIGGER audit_events_no_update
BEFORE UPDATE ON audit_events
FOR EACH ROW EXECUTE FUNCTION audit_events_immutable();

DROP TRIGGER IF EXISTS audit_events_no_delete ON audit_events;
CREATE TRIGGER audit_events_no_delete
BEFORE DELETE ON audit_events
FOR EACH ROW EXECUTE FUNCTION audit_events_immutable();
