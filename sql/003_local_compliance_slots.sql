-- Agent Memoir local-compliance slot schema (portable: PostgreSQL + SQLite)
-- Formatted per .agents/skills/sql-guidelines (case, layout, naming, types).
--
-- Apply order (all steps idempotent):
--   1. 001_core.sql
--   2. SQLite: this file, then 002_vectors_sqlite.sql
--      Postgres: 002_vectors_postgres.sql, then this file
--
-- These slots reserve storage ownership. The local_events table carries the
-- real local event schema, durable_requests the real request schema,
-- copy_inventory and export_destinations the real copy-inventory schemas, and
-- peer_actions the real per-peer request-action schema; subject_family_links,
-- archive_experiments, local_erasure_journal, restore_guards, and
-- erasure_adapters are minimal text-key placeholders that their owner groups
-- will build on later (no feature columns or rows yet).
-- Owner groups:
--   G6    local_events           local event store (content-free local events)
--   G7    subject_family_links   subject-family mapping (stored subject links)
--         durable_requests       durable request state (request schema slot)
--   G19   copy_inventory         copy inventory, staging, custody, product copy result
--         export_destinations    export destination custody and pending delivery state
--   G14D  archive_experiments    old-archive experiment and restore/journal design
--   G14A  local_erasure_journal  reserved id-only slot; the durable erasure journal is an owner-only file outside the store
--   G14I  restore_guards         shared restore refusal and placement guards
--   G8B   erasure_adapters       SQLite/Postgres erasure adapters and retry proof
--         peer_actions           durable per-peer denial rows of the request's enabled peers
--
-- Every statement is idempotent (CREATE TABLE IF NOT EXISTS), so the slot batch
-- can join the open-path migrate batch and re-run on an existing database.

-- ---------------------------------------------------------------------------
-- Slots (one per owner group)
-- ---------------------------------------------------------------------------
-- G6: local event store. One row per protected local action: the intent is the
-- outcome-pending record written before the protected effect, and the outcome
-- write completes it. `seq` is the store-assigned order position (MAX+1 inside
-- the insert transaction, UNIQUE-backed), `scope` is opaque, and `result` is
-- the caller-redacted outcome text (NULL while pending). Pruning removes only
-- completed allowed rows: request (pending) and denial rows always stay.
CREATE TABLE IF NOT EXISTS local_events
(
  action_id                                                                                    text PRIMARY KEY,
  seq                                                                                          bigint NOT NULL UNIQUE CHECK (seq >= 1),
  actor                                                                                        text NOT NULL,
  scope                                                                                        text NOT NULL,
  kind                                                                                         text NOT NULL,
  event_time                                                                                   text NOT NULL,
  outcome_state                                                                                text NOT NULL CHECK (outcome_state IN ('pending', 'allowed', 'denied')),
  result                                                                                       text
  CHECK ((outcome_state = 'pending') = (result IS NULL))
);

-- G7: subject-family mapping (stored subject links).
CREATE TABLE IF NOT EXISTS subject_family_links
(
  id                                                                                           text PRIMARY KEY
);

-- Durable subject-request state. One row per request. `scope` and `record`
-- are the consumer's canonical serializations, stored and compared verbatim;
-- the normalized columns travel with the row so a restart reads them back.
CREATE TABLE IF NOT EXISTS durable_requests
(
  request_id                                                                                   text PRIMARY KEY,
  kind                                                                                         text NOT NULL,
  state                                                                                        text NOT NULL,
  scope                                                                                        text NOT NULL,
  deadline                                                                                     text,
  record                                                                                       text NOT NULL
);

-- G19: copy inventory, staging, custody, product copy result. One row per
-- inventoried copy. The row carries custody and operator-action metadata
-- only: no copy content, no digest of subject data. `request_id` and
-- `custody` are fixed when the copy is inventoried; `exception_reason`
-- names an open gap while the copy has no request result yet.
CREATE TABLE IF NOT EXISTS copy_inventory
(
  copy_id                                                                                      text PRIMARY KEY,
  request_id                                                                                   text NOT NULL,
  custody                                                                                      text NOT NULL,
  owner                                                                                        text NOT NULL,
  required_action                                                                              text NOT NULL,
  state                                                                                        text NOT NULL,
  exception_reason                                                                             text,
  updated_at                                                                                   text NOT NULL
);

-- Export destination custody. One row per export action, keyed by the export
-- action id. The row records the destination path, the custody owner, the
-- required action, and the pending delivery state before the destination file
-- is written, so a crash after the write leaves a known open copy that
-- reconciliation can find by action id. `state` is pending until the delivery
-- completes or fails, and `exception_reason` carries the failure reason.
CREATE TABLE IF NOT EXISTS export_destinations
(
  action_id                                                                                    text PRIMARY KEY,
  destination_path                                                                             text NOT NULL,
  custody_owner                                                                                text NOT NULL,
  required_action                                                                              text NOT NULL,
  state                                                                                        text NOT NULL CHECK (state IN ('pending', 'delivered', 'failed')),
  exception_reason                                                                             text,
  updated_at                                                                                   text NOT NULL
);

-- G14D: old-archive experiment and restore/journal design.
CREATE TABLE IF NOT EXISTS archive_experiments
(
  id                                                                                           text PRIMARY KEY
);

-- G14A: reserved id-only placeholder slot; the durable erasure journal is an owner-only file outside the store.
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

-- Peer-facing request actions. One row per (request, peer, action kind): the
-- intent is written before the peer effect and stays `pending` until the
-- peer's durable proof arrives, so a duplicate or concurrent delivery cannot
-- repeat a completed effect. `state`, `disposition`, and the closed action
-- domain are CHECK-guarded, and the row key plus `action_id` bind every
-- guarded transition. `acknowledged` and `refused` are terminal states, and a
-- refused row that holds no disposition may still record the one evidenced
-- disposition that resolves it: the state stays `refused` and the peer's first
-- evidence is never overwritten.
CREATE TABLE IF NOT EXISTS peer_actions
(
  request_id                                                                                   text NOT NULL,
  peer_id                                                                                      text NOT NULL,
  action_kind                                                                                  text NOT NULL CHECK (action_kind IN ('denial')),
  action_id                                                                                    text NOT NULL,
  state                                                                                        text NOT NULL CHECK (state IN ('pending', 'delivered', 'acknowledged', 'refused')),
  disposition                                                                                  text NOT NULL CHECK (disposition IN ('removal_evidence', 'retention_approved', 'unknown')),
  proof_ref                                                                                    text,
  PRIMARY KEY (request_id, peer_id, action_kind)
);
