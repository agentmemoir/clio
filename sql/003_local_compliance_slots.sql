-- Agent Memoir local-compliance slot schema (portable: PostgreSQL + SQLite)
-- Formatted per .agents/skills/sql-guidelines (case, layout, naming, types).
--
-- Apply order (all steps idempotent):
--   1. 001_core.sql
--   2. SQLite: this file, then 002_vectors_sqlite.sql
--      Postgres: 002_vectors_postgres.sql, then this file
--
-- These slots reserve storage ownership only. Each table is a minimal text-key
-- placeholder that a named owner group will build on later; no feature columns,
-- rows, or triggers live here yet. Owner groups:
--   G6    local_events           local event store (content-free local events)
--   G7    subject_family_links   subject-family mapping (stored subject links)
--   G8A   durable_requests       durable request state (request schema slot)
--   G19   copy_inventory         copy inventory, staging, custody, product copy result
--   G14D  archive_experiments    old-archive experiment and restore/journal design
--   G14A  local_erasure_journal  durable local erasure journal and deny marker
--   G14I  restore_guards         shared restore refusal and placement guards
--   G8B   erasure_adapters       SQLite/Postgres erasure adapters and retry proof
--
-- Every statement is idempotent (CREATE TABLE IF NOT EXISTS), so the slot batch
-- can join the open-path migrate batch and re-run on an existing database.

-- ---------------------------------------------------------------------------
-- Slots (one per owner group)
-- ---------------------------------------------------------------------------
-- G6: local event store (content-free local events).
CREATE TABLE IF NOT EXISTS local_events
(
  id                                                                                           text PRIMARY KEY
);

-- G7: subject-family mapping (stored subject links).
CREATE TABLE IF NOT EXISTS subject_family_links
(
  id                                                                                           text PRIMARY KEY
);

-- G8A: durable request state (request schema slot).
CREATE TABLE IF NOT EXISTS durable_requests
(
  id                                                                                           text PRIMARY KEY
);

-- G19: copy inventory, staging, custody, product copy result.
CREATE TABLE IF NOT EXISTS copy_inventory
(
  id                                                                                           text PRIMARY KEY
);

-- G14D: old-archive experiment and restore/journal design.
CREATE TABLE IF NOT EXISTS archive_experiments
(
  id                                                                                           text PRIMARY KEY
);

-- G14A: durable local erasure journal and deny marker.
CREATE TABLE IF NOT EXISTS local_erasure_journal
(
  id                                                                                           text PRIMARY KEY
);

-- G14I: shared restore refusal and placement guards.
CREATE TABLE IF NOT EXISTS restore_guards
(
  id                                                                                           text PRIMARY KEY
);

-- G8B: SQLite/Postgres erasure adapters and retry proof.
CREATE TABLE IF NOT EXISTS erasure_adapters
(
  id                                                                                           text PRIMARY KEY
);
