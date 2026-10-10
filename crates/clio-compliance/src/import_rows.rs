// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the import planners for entity rows whose
//! batch effects insert their own carrier item row (triples, tasks, failures)
//! and for belief objects restored via `belief_observe`.
//!
//! ## Owns
//! - `plan_triples` / `plan_tasks` / `plan_failures` / `plan_beliefs`
//! - `stageable_carrier` resolution and the carrier preplan + deferral that
//!   keeps carrier rows single-insert
//!
//! ## Does not own
//! - Item/opaque/assoc/persona planning (`import_apply.rs`)
//! - File reading, checksums, and the commit loop (`import.rs`)
//!
//! ## Boundary
//!
//! Changes to row restore semantics (supersede, version, trajectory dedupe)
//! should normally require changes here. Changes to the shared planner state
//! should not.
//!
//! Keep this module focused on carrier-backed row and belief planning.

use clio_store::{BatchWrite, OpaqueItemRecord};
use clio_types::{AmError, BeliefObject, ErrorCode, MemoryItem};
use std::sync::atomic::{AtomicU64, Ordering};

use super::import_apply::Planner;
use crate::bundle::OpaqueItem;
use crate::export_mask::scrub_context;

/// Monotonic suffix so two imported beliefs sharing one timestamp cannot collide.
static IMPORT_BELIEF_SEQ: AtomicU64 = AtomicU64::new(0);

/// Fresh belief id for an imported create (the bundle id is not reused).
fn import_belief_id(bank: &str) -> String {
    let seq = IMPORT_BELIEF_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("bel-imp-{bank}-{nanos}-{seq}")
}

impl Planner<'_> {
    /// Resolve the bundle carrier for a new triple/task/failure row. Returns the
    /// admission-stamped clone planned by `gate_carrier` (the batch executor
    /// re-inserts the carrier row, so a duplicate insert would fail the chunk).
    /// Returns `None` (with an honest note) when the carrier is missing from the
    /// bundle, already exists/staged, or was rejected by the admission gate.
    pub(super) fn stageable_carrier(
        &mut self,
        item_id: &str,
        what: &str,
        key: &str,
    ) -> Result<Option<MemoryItem>, AmError> {
        if let Some(reason) = self.rejected_carriers.get(item_id).cloned() {
            self.skip += 1;
            self.note(format!(
                "{what} {key} restore skipped: carrier item rejected by admission gate ({reason})"
            ));
            return Ok(None);
        }
        if self.item_exists(item_id)? || self.consumed.contains(&format!("carrier:{item_id}")) {
            self.skip += 1;
            self.note(format!(
                "{what} {key} restore skipped: carrier item already exists"
            ));
            return Ok(None);
        }
        let Some(mut carrier) = self.admitted_carriers.get(item_id).cloned() else {
            self.skip += 1;
            self.note(format!(
                "{what} {key} restore skipped: carrier item missing from bundle"
            ));
            return Ok(None);
        };
        self.args.ctx.bank.as_str().clone_into(&mut carrier.bank_id);
        // The carrier is stashed pre-admission, so it never passed the item
        // create path's mask. Scrub it here so it carries the same value the
        // record was masked to; the store fails a record whose context
        // diverges from its carrier, and a hand-crafted bundle must not
        // persist a secret on either side.
        carrier.context = scrub_context(carrier.context.as_deref());
        Ok(Some(carrier))
    }
    /// Cache store lookups and defer carrier items whose row will be inserted
    /// by a `Commit*` executor: the triple/task/failure batch effects re-insert
    /// their carrier item row, so planning the carrier as `CreateItem` too
    /// would double-insert and fail the chunk.
    pub(super) fn preplan(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let entities = &self.envelope.entities;
        for edge in &entities.triples {
            let exists = args.store.get_triple(bank, &edge.id)?.is_some();
            self.edge_seen.insert(edge.id.clone(), exists);
        }
        for task in &entities.tasks {
            let latest = args.history.get_task(bank, &task.task_id)?;
            self.task_latest.insert(task.task_id.clone(), latest);
        }
        for failure in &entities.failures {
            let exists = args.history.get_failure(bank, &failure.id)?.is_some();
            self.failure_exists.insert(failure.id.clone(), exists);
        }
        for edge in &entities.triples {
            if !self.edge_seen[&edge.id] {
                self.defer_carrier(&edge.item_id)?;
            }
        }
        for task in &entities.tasks {
            let stageable = self.task_latest[&task.task_id]
                .as_ref()
                .is_none_or(|latest| latest.version < task.version);
            if stageable {
                self.defer_carrier(&task.item_id)?;
            }
        }
        for failure in &entities.failures {
            if !self.failure_exists[&failure.id] {
                self.defer_carrier(&failure.item_id)?;
            }
        }
        Ok(())
    }

    fn defer_carrier(&mut self, item_id: &str) -> Result<(), AmError> {
        if self.deferred.contains(item_id) {
            return Ok(());
        }
        let missing = !self.item_exists(item_id)?;
        if missing && self.bundle_item(item_id).is_some() {
            self.deferred.insert(item_id.to_owned());
        }
        Ok(())
    }

    pub(super) fn plan_failures(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        for failure in &self.envelope.entities.failures {
            if self
                .failure_exists
                .get(&failure.id)
                .copied()
                .unwrap_or(false)
                || self.consumed.contains(&format!("failure:{}", failure.id))
            {
                self.skip += 1;
                continue;
            }
            let mut record = failure.clone();
            bank.clone_into(&mut record.bank_id);
            // Hand-crafted bundles can carry a secret-shaped context; scrub it
            // rather than trusting that the exporter masked it.
            record.context = scrub_context(record.context.as_deref());
            let Some(carrier) = self.stageable_carrier(&record.item_id, "failure", &record.id)?
            else {
                continue;
            };
            self.consumed.insert(format!("failure:{}", record.id));
            self.consumed.insert(format!("carrier:{}", record.item_id));
            self.staged.push(BatchWrite::CommitFailureRecord {
                item: carrier,
                record,
                actor: actor.to_owned(),
            });
            // The carrier row was already counted by its gated create.
            self.create += 1;
        }
        Ok(())
    }

    pub(super) fn plan_triples(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        // Older tx_from sorts first so closed historical edges insert before
        // the currently-open one; commit_triple_add errors when supersede=false
        // and an open same-subject+predicate edge already exists.
        let mut triples = self.envelope.entities.triples.clone();
        triples.sort_by(|a, b| (&a.tx_from, &a.id).cmp(&(&b.tx_from, &b.id)));
        for edge in &triples {
            if self.edge_seen.get(&edge.id).copied().unwrap_or(false)
                || self.consumed.contains(&format!("triple:{}", edge.id))
            {
                self.skip += 1;
                if args.force && !self.noted_triple_force {
                    self.noted_triple_force = true;
                    self.note(
                        "force does not clobber triples: facts keep bi-temporal identity; \
                         supersession belongs to correct/triple_end"
                            .to_owned(),
                    );
                }
                continue;
            }
            let mut e = edge.clone();
            bank.clone_into(&mut e.bank_id);
            let Some(carrier) = self.stageable_carrier(&e.item_id, "edge", &e.id)? else {
                continue;
            };
            self.consumed.insert(format!("triple:{}", e.id));
            self.consumed.insert(format!("carrier:{}", e.item_id));
            self.staged.push(BatchWrite::CommitTriple {
                item: carrier,
                edge: e,
                supersede: false,
                actor: actor.to_owned(),
            });
            // The carrier row was already counted by its gated create.
            self.create += 1;
        }
        Ok(())
    }

    pub(super) fn plan_tasks(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        for task in &self.envelope.entities.tasks {
            if self
                .task_latest
                .get(&task.task_id)
                .and_then(|latest| latest.as_ref())
                .is_some_and(|latest| latest.version >= task.version)
                || self.consumed.contains(&format!("task:{}", task.task_id))
            {
                self.skip += 1;
                continue;
            }
            let mut record = task.clone();
            bank.clone_into(&mut record.bank_id);
            // Hand-crafted bundles can carry a secret-shaped context; scrub it
            // rather than trusting that the exporter masked it.
            record.context = scrub_context(record.context.as_deref());
            let Some(carrier) = self.stageable_carrier(&record.item_id, "task", &record.task_id)?
            else {
                continue;
            };
            self.consumed.insert(format!("task:{}", record.task_id));
            self.consumed.insert(format!("carrier:{}", record.item_id));
            self.staged.push(BatchWrite::CommitTaskUpsert {
                item: carrier,
                record,
                actor: actor.to_owned(),
            });
            // The carrier row was already counted by its gated create.
            self.create += 1;
        }
        Ok(())
    }

    pub(super) fn plan_beliefs(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        for belief in &self.envelope.entities.beliefs {
            let key = clio_types::proposition_key(&belief.proposition);
            if let Some(existing) = args.belief.find_belief_by_proposition(bank, &key)? {
                let mut appended = 0usize;
                for entry in &belief.confidence_history {
                    let present = existing
                        .confidence_history
                        .iter()
                        .any(|e| e.as_of == entry.as_of && e.evidence_ref == entry.evidence_ref);
                    if present {
                        continue;
                    }
                    self.staged.push(BatchWrite::AppendBeliefConfidence {
                        bank_id: bank.to_owned(),
                        belief_id: existing.id.clone(),
                        entry: entry.clone(),
                        actor: actor.to_owned(),
                    });
                    appended += 1;
                }
                if appended == 0 {
                    self.skip += 1;
                } else {
                    self.create += appended;
                }
                continue;
            }
            let Some(first) = belief.confidence_history.first() else {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "bundle belief has no confidence history to restore",
                ));
            };
            // Score only: the admitted create is staged as an atomic batch effect.
            let decision = clio_belief::score_new_belief(
                bank,
                &belief.proposition,
                first.source_type,
                args.belief_signals,
                &args.policy,
            )?;
            if !decision.pass {
                self.reject_reason(decision.rejection_reason.as_deref());
                continue;
            }
            let created = BeliefObject {
                id: import_belief_id(bank),
                bank_id: bank.to_owned(),
                subject_id: belief.subject_id.clone(),
                proposition: belief.proposition.clone(),
                // A hand-crafted bundle can carry a secret-shaped context that
                // never passed the export scrubber. Scrub it here so no import
                // path can persist an unscrubbed context; an empty result is
                // absent, never `Some("")`.
                context: scrub_context(belief.context.as_deref()),
                // Store-derived at insert; never accepted from a caller, so the
                // bundle's exported `context_as_of` is deliberately dropped
                // here and the store re-derives it from `created_at`. For a
                // belief whose context was later corrected, that makes the
                // restored context visible from creation time rather than from
                // the correction. Recorded as a Known Limitation in the phase
                // file; carrying the exported stamp instead would reopen the
                // path by which sync apply could forge the stamp.
                context_as_of: None,
                confidence_history: belief.confidence_history.clone(),
                created_at: belief.created_at.clone(),
                updated_at: belief.updated_at.clone(),
            };
            self.staged.push(BatchWrite::CreateBelief {
                belief: created,
                actor: actor.to_owned(),
            });
            self.create += belief.confidence_history.len();
        }
        Ok(())
    }
}

/// Bind one opaque bundle row to the restore bank, preserving every metadata field.
pub(super) fn opaque_record(opaque: &OpaqueItem, bank: &str) -> OpaqueItemRecord {
    OpaqueItemRecord {
        id: opaque.id.clone(),
        bank_id: bank.to_owned(),
        subject_id: opaque.subject_id.clone(),
        kind: opaque.kind.clone(),
        category: opaque.category.clone(),
        episodic_type: opaque.episodic_type.clone(),
        epistemic_kind: opaque.epistemic_kind.clone(),
        source_type: opaque.source_type.clone(),
        confidence: opaque.confidence,
        admission_score: opaque.admission_score,
        source_ref: opaque.source_ref.clone(),
        valid_from: opaque.valid_from.clone(),
        valid_until: opaque.valid_until.clone(),
        tx_from: opaque.tx_from.clone(),
        tx_until: opaque.tx_until.clone(),
        created_at: opaque.created_at.clone(),
        updated_at: opaque.updated_at.clone(),
        content_cipher: opaque.content_cipher.clone(),
    }
}
