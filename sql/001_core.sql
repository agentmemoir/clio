-- Agent Memoir core schema (portable: PostgreSQL + SQLite)
-- Formatted per .agents/skills/sql-guidelines (case, layout, naming, types).
--
-- Apply order:
--   1. This file (001_core.sql)
--   2. Dialect vectors:
--        Postgres: 002_vectors_postgres.sql  (Compose mounts both into initdb)
--        SQLite:   002_vectors_sqlite.sql
--
-- Dual-backend type notes (ponytail: ceilings; upgrade path = dialect-only columns):
--   - Timestamps stay text ISO-8601 UTC so SQLite and Postgres share one DDL.
--     Postgres-only TIMESTAMP WITH TIME ZONE + DEFAULT(NOW()) is deferred.
--   - JSON payloads stay text (not jsonb) for the same reason.
--   - Flags stay integer 0/1 (not boolean).
--   - content_ciphertext is text (base64), not bytea/blob.

-- ---------------------------------------------------------------------------
-- Meta
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS schema_settings
(
  key                                                                                          text PRIMARY KEY,
  value                                                                                        text NOT NULL,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text
);

INSERT INTO schema_settings (key, value, created_at, updated_at)
VALUES ('schema_version', '1', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z')
ON CONFLICT (key) DO UPDATE SET
  value = excluded.value,
  updated_at = excluded.updated_at;

-- The active embedding space rows are NOT seeded here: an absent pair means a
-- fresh database, which records the configured (or default) space at open. The
-- store writes `embedding_model`/`embedding_dim` itself.

-- ---------------------------------------------------------------------------
-- Banks (per-repo / per-user / per-agent isolation)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS banks
(
  id                                                                                           text PRIMARY KEY,
  name                                                                                         text,
  meta_json                                                                                    text NOT NULL DEFAULT '{}',
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text
);

-- ---------------------------------------------------------------------------
-- Attribute update-rule registry: discrete vs continuous.
-- attr_class is the `update_rule` declaration; it is never the item
-- epistemic_kind column (vocabulary: separate enums, separate columns).
-- alpha / trend_window / bounds / clamp override the continuous-engine
-- defaults (alpha 0.3, trend_window 10) per attribute when set.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS attribute_schemas
(
  bank_id                                                                                      text NOT NULL,
  attr_key                                                                                     text NOT NULL,
  attr_class                                                                                   text NOT NULL CHECK (attr_class IN ('discrete', 'continuous')),
  alpha                                                                                        numeric,
  trend_window                                                                                 integer,
  bounds_lo                                                                                    numeric,
  bounds_hi                                                                                    numeric,
  clamp_values                                                                                 integer NOT NULL DEFAULT 0 CHECK (clamp_values IN (0, 1)),
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text,
  PRIMARY KEY (bank_id, attr_key)
);

-- ---------------------------------------------------------------------------
-- Continuous state + append-only observation series (EMA trajectories)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS continuous_state
(
  bank_id                                                                                      text NOT NULL,
  attr_key                                                                                     text NOT NULL,
  state                                                                                        numeric NOT NULL,
  trend                                                                                        numeric NOT NULL,
  as_of                                                                                        text NOT NULL,
  -- last N EMA states, oldest → newest (JSON array of finite floats)
  window_json                                                                                  text NOT NULL,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text,
  PRIMARY KEY (bank_id, attr_key)
);

-- Append-only observation history; prior points are never overwritten or deleted.
CREATE TABLE IF NOT EXISTS continuous_observations
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  attr_key                                                                                     text NOT NULL,
  observed_value                                                                               numeric NOT NULL,
  state                                                                                        numeric NOT NULL,
  trend                                                                                        numeric NOT NULL,
  as_of                                                                                        text NOT NULL,
  actor                                                                                        text NOT NULL,
  created_at                                                                                   text NOT NULL
);

CREATE INDEX IF NOT EXISTS continuous_observations_key_asof_inx
  ON continuous_observations (bank_id, attr_key, as_of);

-- ---------------------------------------------------------------------------
-- Memory items (semantic + episodic units)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS items
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  -- semantic: task_spec|schema|tool_config|output_constraint|persona
  -- episodic type tag: triple|gist|task|failure|temporal (belief is not a tag)
  kind                                                                                         text NOT NULL CHECK (kind IN ('semantic', 'episodic')),
  category                                                                                     text,
  episodic_type                                                                                text,
  epistemic_kind                                                                               text NOT NULL CHECK (epistemic_kind IN ('fact', 'belief')),
  -- present when epistemic_kind=belief; NULL for facts (reject-if-present)
  source_type                                                                                  text CHECK (source_type IS NULL OR source_type IN ('user_stated', 'agent_inferred', 'third_party')),
  confidence                                                                                   numeric,
  admission_score                                                                              numeric,
  source_ref                                                                                   text,
  -- unused plaintext slots; snapshot/gist halves live inside content_ciphertext
  snapshot_json                                                                                text,
  snapshot_verified                                                                            integer NOT NULL DEFAULT 0 CHECK (snapshot_verified IN (0, 1)),
  gist_text                                                                                    text,
  -- DEK envelope: opaque bytes or v1 JSON {snapshot,gist} plaintext before seal
  content_ciphertext                                                                           text,
  subject_id                                                                                   text,
  -- bi-temporal
  valid_from                                                                                   text NOT NULL,
  valid_until                                                                                  text,
  tx_from                                                                                      text NOT NULL,
  tx_until                                                                                     text,
  -- operations removal not compliance erasure
  discarded_at                                                                                 text,
  discard_reason                                                                               text,
  -- hygiene archive: reversible exclusion from default retrieve;
  -- never a compliance erasure marker and never a DEK/tombstone signal
  archived_at                                                                                  text,
  -- correction lineage: corrected assertion links its prior;
  -- superseded assertion points at its replacement. Never deleted.
  corrects_item_id                                                                             text,
  superseded_by_id                                                                             text,
  -- hygiene
  hygiene_flag                                                                                 text,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text NOT NULL,
  CHECK
  (
    (
      kind = 'semantic' AND category IN
      (
       'task_spec', 'schema', 'tool_config', 'output_constraint', 'persona'
      ) AND episodic_type IS NULL
    )
    OR
    (
      kind = 'episodic' AND category IS NULL AND episodic_type IN
      (
       'triple', 'gist', 'task', 'failure', 'temporal'
      )
    )
  ),
  CHECK
  (
    (epistemic_kind = 'fact' AND source_type IS NULL)
    OR
    (epistemic_kind = 'belief' AND source_type IS NOT NULL)
  )
);

CREATE INDEX IF NOT EXISTS items_bank_id_inx
ON items (bank_id);

CREATE INDEX IF NOT EXISTS items_bank_id_kind_category_inx
ON items (bank_id, kind, category);

CREATE INDEX IF NOT EXISTS items_bank_id_episodic_type_inx
ON items (bank_id, episodic_type);

CREATE INDEX IF NOT EXISTS items_bank_id_valid_from_valid_until_inx
ON items (bank_id, valid_from, valid_until);

CREATE INDEX IF NOT EXISTS items_bank_id_tx_from_tx_until_inx
ON items (bank_id, tx_from, tx_until);

-- Provenance-class filtering: facts are exactly the `source_type IS NULL`
-- rows, so the index is not partial and serves the fact rung and the three
-- belief rungs beside the bank scope.
CREATE INDEX IF NOT EXISTS items_bank_id_source_type_inx
ON items (bank_id, source_type);

CREATE INDEX IF NOT EXISTS items_subject_id_inx
ON items (subject_id);

-- NOTE: the correction-lineage indexes (`corrects_item_id`, `superseded_by_id`)
-- live in the per-dialect 002 files: they must be created only after the
-- columns exist, including on pre-existing databases that converge the columns
-- via ALTER (Postgres) or the SQLite convergence hook.

CREATE INDEX IF NOT EXISTS items_bank_id_active_inx
ON items (bank_id)
WHERE discarded_at IS NULL;

-- Newest-first window scans (admission neighbor/recency signals). Covers the
-- exact predicate and ordering so a bounded recent-window read avoids a temp
-- sort over the whole bank.
CREATE INDEX IF NOT EXISTS items_bank_id_created_at_inx
ON items (bank_id, created_at, id)
WHERE discarded_at IS NULL AND tx_until IS NULL;

-- ---------------------------------------------------------------------------
-- Index pipeline queue: durable outbox of leaves awaiting dense/lexical
-- indexing. One row per (bank, item); the index worker drains it off the write
-- path. Derived state — safe to wipe and rebuild from items.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS index_pending
(
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text NOT NULL,
  attempts                                                                                     integer NOT NULL DEFAULT 0,
  last_error                                                                                   text,
  last_attempt_at                                                                              text,
  queued_at                                                                                    text NOT NULL,
  updated_at                                                                                   text NOT NULL,
  PRIMARY KEY (bank_id, item_id)
);

CREATE INDEX IF NOT EXISTS index_pending_queued_at_inx
ON index_pending (queued_at);

-- ---------------------------------------------------------------------------
-- Episodic triples

-- Identity is (bank_id, subject, predicate), so `subject` and
-- `predicate` are bi-temporal STRUCTURE and stay in plaintext. The asserted
-- VALUE is content: `object_ciphertext` holds it sealed under the carrier
-- item's subject DEK, matching the `beliefs` treatment of proposition text
-- (field-level encryption). The value is unreadable without the DEK,
-- so destroying that DEK on a verified erasure renders it unrecoverable.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS triples
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text NOT NULL REFERENCES items (id),
  subject                                                                                      text NOT NULL,
  predicate                                                                                    text NOT NULL,
  object_ciphertext                                                                            text NOT NULL,
  epistemic_kind                                                                               text NOT NULL CHECK (epistemic_kind IN ('fact', 'belief')),
  source_type                                                                                  text,
  confidence                                                                                   numeric,
  valid_from                                                                                   text NOT NULL,
  valid_until                                                                                  text,
  tx_from                                                                                      text NOT NULL,
  tx_until                                                                                     text,
  snapshot_ref                                                                                 text,
  gist_ref                                                                                     text,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text
);

CREATE INDEX IF NOT EXISTS triples_bank_id_subject_predicate_inx
ON triples (bank_id, subject, predicate);

CREATE INDEX IF NOT EXISTS triples_bank_id_subject_predicate_open_inx
ON triples (bank_id, subject, predicate)
WHERE valid_until IS NULL;

-- The open-edge identity invariant is a database constraint: at most one open
-- (valid_until IS NULL AND tx_until IS NULL) triple per (bank_id, subject,
-- predicate). The unique partial index replaces the former non-unique
-- triples_bank_id_subject_predicate_open_both_inx, which served no code query
-- and is dropped here after plan confirmation; a legacy duplicate would fail
-- the CREATE UNIQUE INDEX build, so the open path pre-flights duplicates first
-- (see triple_invariant in clio-store) and fails with a named diagnostic.
DROP INDEX IF EXISTS triples_bank_id_subject_predicate_open_both_inx;

CREATE UNIQUE INDEX IF NOT EXISTS triples_open_uniq
ON triples (bank_id, subject, predicate)
WHERE valid_until IS NULL AND tx_until IS NULL;

CREATE INDEX IF NOT EXISTS triples_bank_id_valid_from_valid_until_inx
ON triples (bank_id, valid_from, valid_until);

CREATE INDEX IF NOT EXISTS triples_bank_id_tx_from_tx_until_inx
ON triples (bank_id, tx_from, tx_until);

-- ---------------------------------------------------------------------------
-- Task history. Task records are versioned per (bank, task_id)
-- each version is carried by an episodic `task` item so retrieval domains can
-- select it. Definition/steps CONTENT is sealed under the subject DEK; only the
-- key columns and structure are plaintext (field-level encryption).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS task_records
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text NOT NULL REFERENCES items (id),
  task_id                                                                                      text NOT NULL,
  subject_id                                                                                   text NOT NULL,
  version                                                                                      integer NOT NULL CHECK (version >= 1),
  status                                                                                       text NOT NULL,
  definition_cipher                                                                            text NOT NULL,
  steps_cipher                                                                                 text NOT NULL,
  evidence_ref                                                                                 text,
  admission_score                                                                              numeric NOT NULL CHECK (admission_score >= 0 AND admission_score <= 1),
  created_at                                                                                   text NOT NULL,
  UNIQUE (bank_id, task_id, version)
);

CREATE INDEX IF NOT EXISTS task_records_bank_id_task_id_version_inx
ON task_records (bank_id, task_id, version);

-- ---------------------------------------------------------------------------
-- Failure history. `attempt_n` is unique per (bank, task_id), so
-- re-recording the same attempt is rejected rather than silently duplicated.
-- `lesson` is length-capped at the tool boundary before storage.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS failure_records
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text NOT NULL REFERENCES items (id),
  task_id                                                                                      text NOT NULL,
  subject_id                                                                                   text NOT NULL,
  attempt_n                                                                                    integer NOT NULL CHECK (attempt_n >= 1),
  what_failed_cipher                                                                           text NOT NULL,
  lesson_cipher                                                                                text NOT NULL,
  lesson_tokens                                                                                integer NOT NULL CHECK (lesson_tokens >= 0),
  evidence_ref                                                                                  text,
  admission_score                                                                              numeric NOT NULL CHECK (admission_score >= 0 AND admission_score <= 1),
  epistemic_kind                                                                               text NOT NULL CHECK (epistemic_kind = 'belief'),
  created_at                                                                                   text NOT NULL,
  UNIQUE (bank_id, task_id, attempt_n)
);

CREATE INDEX IF NOT EXISTS failure_records_bank_id_task_id_inx
ON failure_records (bank_id, task_id);

-- ---------------------------------------------------------------------------
-- Beliefs + append-only confidence history

-- Beliefs are belief objects: NOT items, NOT a semantic category, and
-- NOT an episodic type tag. Identity is (bank_id, proposition_key), where
-- proposition_key is the documented normalization of the proposition text.
-- The proposition CONTENT is sealed under the subject DEK; only the normalized
-- identity key and structure are plaintext (field-level encryption).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS beliefs
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  subject_id                                                                                   text NOT NULL,
  proposition_key                                                                              text NOT NULL,
  proposition_ciphertext                                                                       text NOT NULL,
  -- Optional bounded descriptive source `context`, sealed under the subject DEK
  -- NULL when the record was written without context.
  context_cipher                                                                               text,
  -- Effective time of the currently stored sealed context: the belief's
  -- creation instant when the context arrived at creation, the correction stamp
  -- when it arrived through an authorized correction. NULL on a legacy row or a
  -- belief written without context. Point-in-time reads gate on it.
  context_as_of                                                                                text,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text
);

CREATE UNIQUE INDEX IF NOT EXISTS beliefs_bank_id_proposition_key_uix
ON beliefs (bank_id, proposition_key);

-- Append-only: no UPDATE/DELETE of historical confidence rows.
CREATE TABLE IF NOT EXISTS belief_confidence_entries
(
  id                                                                                           text PRIMARY KEY,
  belief_id                                                                                    text NOT NULL REFERENCES beliefs (id),
  as_of                                                                                        text NOT NULL,
  confidence                                                                                   numeric NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
  evidence_ref                                                                                 text NOT NULL,
  source_type                                                                                  text NOT NULL CHECK (source_type IN ('user_stated', 'agent_inferred', 'third_party')),
  created_at                                                                                   text NOT NULL,
  UNIQUE (belief_id, as_of, id)
);

CREATE INDEX IF NOT EXISTS belief_confidence_entries_belief_id_as_of_inx
ON belief_confidence_entries (belief_id, as_of);

-- ---------------------------------------------------------------------------
-- Persona: stable = discrete; preferences = continuous EMA
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS persona_stable_entries
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  attr_key                                                                                     text NOT NULL,
  value_cipher                                                                                   text NOT NULL,
  -- Optional bounded descriptive source `context`, sealed under the bank DEK
  -- NULL when the record version was written without context.
  context_cipher                                                                                 text,
  confidence                                                                                   numeric,
  source                                                                                       text NOT NULL,
  source_ref                                                                                   text,
  admission_score                                                                              numeric NOT NULL CHECK (admission_score >= 0 AND admission_score <= 1),
  valid_from                                                                                   text NOT NULL,
  valid_until                                                                                  text,
  tx_from                                                                                      text NOT NULL,
  tx_until                                                                                     text,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text
);

-- One open (valid) version per (bank, attr_key): invalidation semantics.
-- The unique index doubles as the open-rows lookup index; no second index.
CREATE UNIQUE INDEX IF NOT EXISTS persona_stable_entries_bank_id_attr_key_open_uix
ON persona_stable_entries (bank_id, attr_key)
WHERE valid_until IS NULL;

CREATE TABLE IF NOT EXISTS persona_preferences
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  attr_key                                                                                     text NOT NULL,
  value                                                                                        numeric NOT NULL,
  trend                                                                                        numeric,
  -- Optional bounded descriptive source `context`, sealed under the bank DEK
  -- NULL when no observation carried a context yet.
  context_cipher                                                                               text,
  -- Timestamp at which the currently stored sealed `context` became effective
  -- A write that supplies a context sets it to that write's `as_of`;
  -- an ordinary update preserves it with the context. NULL on a legacy row,
  -- where readers fall back to this row's `as_of`.
  context_as_of                                                                                text,
  confidence                                                                                   numeric,
  source                                                                                       text NOT NULL,
  source_ref                                                                                   text,
  admission_score                                                                              numeric NOT NULL CHECK (admission_score >= 0 AND admission_score <= 1),
  as_of                                                                                        text NOT NULL,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text NOT NULL,
  UNIQUE (bank_id, attr_key)
);

-- ---------------------------------------------------------------------------
-- Tasks + failures
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tasks
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text,
  definition                                                                                   text NOT NULL,
  status                                                                                       text,
  steps_json                                                                                   text NOT NULL DEFAULT '[]',
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text NOT NULL
);

CREATE INDEX IF NOT EXISTS tasks_bank_id_inx
ON tasks (bank_id);

CREATE TABLE IF NOT EXISTS failures
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text,
  task_id                                                                                      text NOT NULL REFERENCES tasks (id),
  attempt_n                                                                                    integer NOT NULL,
  what_failed                                                                                  text NOT NULL,
  lesson                                                                                       text NOT NULL,
  evidence_ref                                                                                 text,
  epistemic_kind                                                                               text NOT NULL DEFAULT 'belief' CHECK (epistemic_kind IN ('fact', 'belief')),
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text
);

CREATE INDEX IF NOT EXISTS failures_bank_id_task_id_inx
ON failures (bank_id, task_id);

-- ---------------------------------------------------------------------------
-- MemTree: leaves + dirty ancestor refresh
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS memtree_nodes
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  parent_id                                                                                    text,
  -- null for internal aggregates; set for leaf nodes referencing a memory item
  item_id                                                                                      text,
  -- 0 = leaf; increases toward root
  depth                                                                                        integer NOT NULL DEFAULT 0,
  interval_start                                                                               text NOT NULL,
  interval_end                                                                                 text NOT NULL,
  -- ordered child node ids (JSON array); empty for leaves
  child_ids_json                                                                               text NOT NULL DEFAULT '[]',
  summary_gist                                                                                 text,
  dirty                                                                                        integer NOT NULL DEFAULT 0 CHECK (dirty IN (0, 1)),
  generation                                                                                   integer NOT NULL DEFAULT 0,
  refreshing                                                                                   integer NOT NULL DEFAULT 0 CHECK (refreshing IN (0, 1)),
  task_id                                                                                      text,
  last_error                                                                                   text,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text NOT NULL
);

CREATE INDEX IF NOT EXISTS memtree_nodes_bank_id_parent_id_inx
ON memtree_nodes (bank_id, parent_id);

CREATE INDEX IF NOT EXISTS memtree_nodes_bank_id_dirty_inx
ON memtree_nodes (bank_id)
WHERE dirty = 1;

CREATE INDEX IF NOT EXISTS memtree_nodes_item_id_inx
ON memtree_nodes (item_id);

CREATE INDEX IF NOT EXISTS memtree_nodes_bank_id_interval_inx
ON memtree_nodes (bank_id, interval_start, interval_end);

-- ---------------------------------------------------------------------------
-- Association edges: assoc_edge in graph/structure metadata
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS assoc_edges
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  source_id                                                                                    text NOT NULL,
  target_id                                                                                    text NOT NULL,
  -- coactivation | explicit (hub distillation is a write-path item, not an assoc_kind)
  assoc_kind                                                                                   text NOT NULL CHECK (assoc_kind IN ('coactivation', 'explicit')),
  relationship                                                                                 text,
  weight                                                                                       numeric NOT NULL DEFAULT 0 CHECK (weight >= 0 AND weight <= 1),
  last_updated_at                                                                              text NOT NULL,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text,
  UNIQUE (bank_id, source_id, target_id, assoc_kind, relationship)
);

CREATE INDEX IF NOT EXISTS assoc_edges_bank_id_source_id_inx
ON assoc_edges (bank_id, source_id);

CREATE INDEX IF NOT EXISTS assoc_edges_bank_id_target_id_inx
ON assoc_edges (bank_id, target_id);

-- One undirected co-activation weight per unordered pair. The table-level
-- UNIQUE includes `relationship`, which is NULL for coactivation, so it does
-- not constrain these rows; this partial index does.
CREATE UNIQUE INDEX IF NOT EXISTS assoc_edges_coactivation_pair_inx
ON assoc_edges (bank_id, source_id, target_id)
WHERE assoc_kind = 'coactivation';

-- ---------------------------------------------------------------------------
-- Audit / telemetry + hygiene log
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS audit_events
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text,
  operation                                                                                    text NOT NULL,
  item_id                                                                                      text,
  category                                                                                     text,
  epistemic_kind                                                                               text,
  confidence                                                                                   numeric,
  admission_score                                                                              numeric,
  actor                                                                                        text NOT NULL,
  hygiene_action                                                                               text,
  detail_json                                                                                  text NOT NULL DEFAULT '{}',
  created_at                                                                                   text NOT NULL
);

CREATE INDEX IF NOT EXISTS audit_events_item_id_created_at_inx
ON audit_events (item_id, created_at);

CREATE INDEX IF NOT EXISTS audit_events_bank_id_created_at_inx
ON audit_events (bank_id, created_at);

CREATE TABLE IF NOT EXISTS hygiene_audit_logs
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text NOT NULL,
  noise_score                                                                                  numeric,
  action                                                                                       text NOT NULL CHECK (action IN ('flag', 'archive', 'discard', 'keep', 'delete')),
  actor                                                                                        text NOT NULL,
  confirmed                                                                                    integer NOT NULL DEFAULT 0 CHECK (confirmed IN (0, 1)),
  dry_run                                                                                      integer NOT NULL DEFAULT 0 CHECK (dry_run IN (0, 1)),
  detail_json                                                                                  text NOT NULL DEFAULT '{}',
  created_at                                                                                   text NOT NULL
);

CREATE INDEX IF NOT EXISTS hygiene_audit_logs_bank_id_created_at_inx
ON hygiene_audit_logs (bank_id, created_at);

-- ---------------------------------------------------------------------------
-- Compliance tombstones — content-free
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tombstones
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  item_id_hash                                                                                 text NOT NULL,
  category                                                                                     text,
  reason_code                                                                                  text NOT NULL,
  legal_basis                                                                                  text,
  request_id                                                                                   text,
  created_at                                                                                   text NOT NULL
);

CREATE INDEX IF NOT EXISTS tombstones_bank_id_created_at_inx
ON tombstones (bank_id, created_at);

-- ---------------------------------------------------------------------------
-- Compliance tombstones — content-free records of crypto-shredding.
-- Subject and item references are one-way SHA-256 hashes; no plaintext,
-- normalized tokens, or reversible values are ever stored here.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS compliance_tombstones
(
  id                                                                                           text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  request_id                                                                                   text NOT NULL,
  legal_basis                                                                                  text NOT NULL,
  subject_hash                                                                                 text NOT NULL,
  item_id_hash                                                                                 text,
  category                                                                                     text,
  reason_code                                                                                  text NOT NULL,
  deleted_at                                                                                   text NOT NULL
);

CREATE INDEX IF NOT EXISTS compliance_tombstones_bank_id_request_inx
ON compliance_tombstones (bank_id, request_id);

-- ---------------------------------------------------------------------------
-- DEK references only — never store raw DEK material here
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS subject_keys
(
  subject_id                                                                                   text PRIMARY KEY,
  dek_ref                                                                                      text NOT NULL,
  status                                                                                       text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'shredded')),
  shredded_at                                                                                  text,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text
);

-- ---------------------------------------------------------------------------
-- Sync cursors (additive) — harmless on single-host
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS sync_cursors
(
  bank_id                                                                                      text NOT NULL,
  peer_id                                                                                      text NOT NULL,
  cursor_token                                                                                 text NOT NULL,
  created_at                                                                                   text NOT NULL,
  updated_at                                                                                   text NOT NULL,
  PRIMARY KEY (bank_id, peer_id)
);

-- ---------------------------------------------------------------------------
-- Sync protocol (additive) — harmless on single-host.
-- sync_cursors above stores per-(bank, peer/direction) opaque cursor tokens;
-- sync_mutations is the durable per-bank journal (seq = commit-ordered
-- position within the bank); sync_conflicts journals LWW resolutions;
-- sync_dead_letter quarantines permanently unapplicable mutations until an
-- operator ack-skip.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS sync_mutations
(
  bank_id                                                                                      text NOT NULL,
  seq                                                                                          integer NOT NULL CHECK (seq >= 0),
  sync_mutation_id                                                                             text NOT NULL,
  entity_kind                                                                                  text NOT NULL CHECK (entity_kind IN ('item', 'triple', 'assoc_edge', 'persona', 'task', 'failure', 'belief', 'tombstone_meta')),
  entity_id                                                                                    text NOT NULL,
  op                                                                                           text NOT NULL CHECK (op IN ('upsert', 'invalidate', 'discard', 'archive', 'reinforce', 'prune')),
  transaction_time                                                                             text NOT NULL,
  device_id                                                                                    text NOT NULL,
  content_hash                                                                                 text NOT NULL,
  -- Plaintext mode: CiphertextEnvelope JSON sealed under this node's local
  -- DEKs. Client-encrypted mode: opaque client ciphertext JSON (server-blind).
  content_sealed                                                                               text,
  encrypted                                                                                    integer NOT NULL DEFAULT 0 CHECK (encrypted IN (0, 1)),
  admission_receipt                                                                            text,
  created_at                                                                                   text NOT NULL,
  PRIMARY KEY (bank_id, sync_mutation_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS sync_mutations_bank_id_seq_uq
  ON sync_mutations (bank_id, seq);

-- Secondary idempotence guard: same logical content + op already applied.
CREATE INDEX IF NOT EXISTS sync_mutations_apply_guard_inx
  ON sync_mutations (bank_id, entity_kind, entity_id, op, content_hash);

CREATE TABLE IF NOT EXISTS sync_conflicts
(
  bank_id                                                                                      text NOT NULL,
  entity_kind                                                                                  text NOT NULL,
  entity_id                                                                                    text NOT NULL,
  winner_mutation_id                                                                           text NOT NULL,
  loser_mutation_id                                                                            text NOT NULL,
  winner_transaction_time                                                                      text NOT NULL,
  loser_transaction_time                                                                       text NOT NULL,
  winner_device_id                                                                             text NOT NULL,
  loser_device_id                                                                              text NOT NULL,
  created_at                                                                                   text NOT NULL,
  PRIMARY KEY (bank_id, loser_mutation_id)
);

CREATE TABLE IF NOT EXISTS sync_dead_letter
(
  sync_mutation_id                                                                             text PRIMARY KEY,
  bank_id                                                                                      text NOT NULL,
  reason_code                                                                                  text NOT NULL,
  detail                                                                                       text NOT NULL,
  envelope_json                                                                                text NOT NULL,
  status                                                                                       text NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'acked_skip')),
  first_seen                                                                                   text NOT NULL,
  last_error                                                                                   text,
  updated_at                                                                                   text
);

-- ---------------------------------------------------------------------------
-- Persona-context erasure tombstone: a verified compliance
-- erasure purges the persona context of every bank it touches (persona rows
-- carry no subject attribution, so the purge is bank-scoped). The tombstone
-- row records that purge per bank and fail-closes later persona writes into
-- that bank, so purged context can never be regenerated — locally or through
-- a delayed sync mutation.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS persona_erase_tombstones
(
  bank_id                                                                                      text PRIMARY KEY,
  erased_at                                                                                    text NOT NULL
);

-- ---------------------------------------------------------------------------
-- Distilled units: one derivation record per hub-distilled item, keyed by the
-- distilled item's own id. Ids and timestamps only: no content, gist, or key
-- material. The claim set (hub id + source item ids) is the authority; the
-- item snapshot is a projection written from the same cluster state in the
-- same transaction. `record_version` is the extension point for later fields.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS distilled_units
(
  bank_id                                                                                      text NOT NULL,
  item_id                                                                                      text NOT NULL,
  hub_id                                                                                       text NOT NULL,
  source_ids_json                                                                              text NOT NULL DEFAULT '[]',
  marker                                                                                       text NOT NULL,
  record_version                                                                               integer NOT NULL DEFAULT 1 CHECK (record_version >= 1),
  distilled_at                                                                                 text NOT NULL,
  -- Reconciliation state (record shape v2): the current revision number (1 =
  -- the first write) and the stale state. A stale unit is not current in
  -- recall until reconciliation rebuilds it; the reason codes are
  -- `source_removed` (operations-removal cascade) and `revision_rejected`
  -- (admission refused a revision). Pre-v2 rows read revision 1 and no stale
  -- state. Ids and codes only: no content, gist, or key material.
  revision                                                                                     integer NOT NULL DEFAULT 1 CHECK (revision >= 1),
  stale_reason                                                                                 text,
  stale_detail                                                                                 text,
  stale_at                                                                                     text,
  -- Per-revision evidence trail (record shape v3): one list per revision, each
  -- entry naming a claimed source id and that source's last-change time. Ids
  -- and timestamps only: no content, gist, or key material, so the list never
  -- needs its own shredding and survives exactly as long as the row's metadata
  -- does. Empty on a pre-v3 row; a no-op wave appends nothing. See
  -- `distilled_evidence` for the shape and `distilled_freshness` for the
  -- read-time staleness decision taken from it.
  evidence_json                                                                               text NOT NULL DEFAULT '[]',
  PRIMARY KEY (bank_id, item_id)
);
-- ---------------------------------------------------------------------------
-- Distill intents: one row per bank and hub, naming the wave that claimed the hub before it distils it, so an interrupted wave is recognisable.
-- Ids and timestamps only: no content, gist, snapshot, or key material.
-- The row is written once; a second claim for the same pair changes nothing, and completion records the produced revision rather than downgrading the claim.
-- The row is not deleted on its own: the per-item erasure path removes it, in the same transaction, with the hub or the distilled unit it names, and the compliance erasure path retains it, so the claim follows those rows in both.
-- `revision_item_id` is empty until the wave completes and then holds the revision it produced, which is the whole of the completion state.
-- `claimed_at` uses the same stamp format as a derivation record's `distilled_at`.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS distill_intents
(
  bank_id                                                                                      text NOT NULL,
  hub_id                                                                                       text NOT NULL,
  unit_item_id                                                                                 text NOT NULL,
  actor                                                                                        text NOT NULL,
  claimed_at                                                                                   text NOT NULL,
  revision_item_id                                                                             text NOT NULL,
  PRIMARY KEY (bank_id, hub_id)
);
