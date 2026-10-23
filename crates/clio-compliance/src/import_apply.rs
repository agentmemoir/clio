// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-entity import planner shared by the
//! `import` entrypoint: it classifies every bundle entity as create / skip /
//! overwrite / reject and stages the batch writes for live commit.
//!
//! ## Owns
//! - The `Planner` state machine over the bundle's entity arrays
//! - Item / opaque / assoc planning and the shared planner state
//! - Carrier deferral so a carrier row is inserted exactly once
//! - Idempotency dedupe rules per entity kind
//!
//! ## Does not own
//! - Persona row planning (`import_apply_persona.rs`)
//! - Triple/task/failure/belief planning (`import_rows.rs`)
//! - File reading, checksums, and the commit loop (`import.rs`)
//! - Store SQL or admission math
//!
//! ## Boundary
//!
//! Changes to planning order or dedupe rules should normally require changes
//! here. Changes to the file contract or the report shape should not.
//!
//! Keep this module focused on import planning.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use clio_admission::gated::{GateRequest, gated_create_raw};
use clio_config::secret::{masked_clone, scrub_inline_secrets};
use clio_store::BatchWrite;
use clio_types::{AmError, ItemKind, MemoryItem, SemanticCategory, TaskRecord};
use serde_json::json;

use super::ImportArgs;
use super::import_rows::opaque_record;
use crate::bundle::BundleEnvelope;
use crate::export_mask::scrub_context;

// Persona row planning lives in its own module (extracted to keep this file
// within the source-size bound); it shares the same `Planner` state.
#[path = "import_apply_persona.rs"]
mod persona;

/// Per-entity import planner (see `run` for the entity order).
pub(super) struct Planner<'a> {
    pub(super) args: &'a ImportArgs<'a>,
    pub(super) envelope: &'a BundleEnvelope,
    pub(super) staged: Vec<BatchWrite>,
    pub(super) create: usize,
    pub(super) skip: usize,
    pub(super) reject: usize,
    pub(super) overwrite: usize,
    pub(super) samples: Vec<String>,
    pub(super) notes: Vec<String>,
    pub(super) item_exists: HashMap<String, bool>,
    pub(super) edge_seen: HashMap<String, bool>,
    pub(super) task_latest: HashMap<String, Option<TaskRecord>>,
    pub(super) failure_exists: HashMap<String, bool>,
    pub(super) deferred: HashSet<String>,
    pub(super) consumed: HashSet<String>,
    /// Admission-stamped carriers for rows whose executor re-inserts them.
    pub(super) admitted_carriers: HashMap<String, MemoryItem>,
    /// Carriers rejected by the admission gate (reason per carrier id).
    pub(super) rejected_carriers: HashMap<String, String>,
    pub(super) noted_triple_force: bool,
}

impl<'a> Planner<'a> {
    pub(super) fn new(args: &'a ImportArgs<'a>, envelope: &'a BundleEnvelope) -> Self {
        Self {
            args,
            envelope,
            staged: Vec::new(),
            create: 0,
            skip: 0,
            reject: 0,
            overwrite: 0,
            samples: Vec::new(),
            notes: Vec::new(),
            item_exists: HashMap::new(),
            edge_seen: HashMap::new(),
            task_latest: HashMap::new(),
            failure_exists: HashMap::new(),
            deferred: HashSet::new(),
            consumed: HashSet::new(),
            admitted_carriers: HashMap::new(),
            rejected_carriers: HashMap::new(),
            noted_triple_force: false,
        }
    }

    /// Plan every entity array in payload order.
    ///
    /// # Errors
    /// Store read failures or admission-signal failures.
    pub(super) fn run(&mut self) -> Result<(), AmError> {
        self.preplan()?;
        self.plan_items()?;
        self.plan_opaque()?;
        self.plan_triples()?;
        self.plan_assoc()?;
        self.plan_stable()?;
        self.plan_preferences()?;
        self.plan_tasks()?;
        self.plan_failures()?;
        self.plan_beliefs()?;
        self.plan_tombstones();
        Ok(())
    }

    pub(super) fn note(&mut self, text: String) {
        if self.notes.len() < 9 {
            self.notes.push(text);
        } else if self.notes.last().map(String::as_str) != Some("(further notes truncated)") {
            self.notes.push("(further notes truncated)".to_owned());
        }
    }

    /// Stage the `import_force` audit row inside the same batch as the overwrite.
    ///
    /// `item_id` is the account the overwrite is attributed to: the ledger item
    /// id for items/edges, and the persona handle (`persona-stable:<key>` /
    /// `persona-preference:<key>`, the shape the persona store audits with) for
    /// persona rows, which have no ledger item.
    pub(super) fn stage_force_audit(&mut self, bank: &str, item_id: &str, actor: &str) {
        let detail = json!({
            "reason": self.args.reason.unwrap_or(""),
            "source": "import"
        });
        self.staged.push(BatchWrite::AuditEvent {
            bank_id: bank.to_owned(),
            operation: "import_force".into(),
            item_id: Some(item_id.to_owned()),
            actor: actor.to_owned(),
            detail,
        });
    }

    pub(super) fn reject_reason(&mut self, reason: Option<&str>) {
        self.reject += 1;
        if self.samples.len() < 10 {
            let masked = scrub_inline_secrets(reason.unwrap_or("admission gate rejection"));
            self.samples.push(masked);
        }
    }

    pub(super) fn item_exists(&mut self, id: &str) -> Result<bool, AmError> {
        if let Some(seen) = self.item_exists.get(id) {
            return Ok(*seen);
        }
        let args = self.args;
        let exists = args
            .store
            .get_memory_item(args.ctx.bank.as_str(), id)?
            .is_some();
        self.item_exists.insert(id.to_owned(), exists);
        Ok(exists)
    }

    pub(super) fn bundle_item(&self, id: &str) -> Option<&MemoryItem> {
        self.envelope.entities.items.iter().find(|i| i.id == id)
    }

    /// True when `id` names a row the bundle itself carries — an item or an
    /// opaque (`content_mode = ciphertext_backup`) row — so a pointer at it has
    /// to be re-keyed alongside the row. `ciphertext_backup` bundles carry an
    /// empty `items` array and put every row in `opaque_items`, so both arrays
    /// have to count.
    pub(super) fn bundle_owns(&self, id: &str) -> bool {
        self.bundle_item(id).is_some()
            || self
                .envelope
                .entities
                .opaque_items
                .iter()
                .any(|o| o.id == id)
    }

    fn plan_items(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        for item in &self.envelope.entities.items {
            // Portable bundles may target another bank; every entity is
            // restored into ctx.bank, so the entity bank_id is rewritten here.
            let mut it = item.clone();
            bank.clone_into(&mut it.bank_id);
            let id = import_id(bank, &item.bank_id, &item.id).into_owned();
            it.id.clone_from(&id);
            if self.item_exists(&id)? {
                if args.force {
                    self.overwrite += 1;
                    let masked = masked_item(&it);
                    self.staged.push(BatchWrite::UpsertMemoryItem {
                        item: masked,
                        actor: actor.to_owned(),
                    });
                    self.stage_force_audit(bank, &id, actor);
                } else {
                    self.skip += 1;
                }
                continue;
            }
            // Deferred carriers still pass the gates here; the
            // Commit* row executor later inserts the admitted (score-stamped)
            // clone instead of a CreateItem effect.
            if self.deferred.contains(&item.id) {
                self.gate_carrier(&it)?;
                continue;
            }
            self.gate_new_item(&it)?;
        }
        Ok(())
    }

    /// Gated create for a standalone item: the admitted clone is staged as a
    /// `CreateItem` batch effect.
    fn gate_new_item(&mut self, it: &MemoryItem) -> Result<(), AmError> {
        let args = self.args;
        let kind_key_pair = kind_key(it);
        let actor = args.ctx.actor.as_str().to_owned();
        let staged = &mut self.staged;
        let decision = gated_create_raw(
            &GateRequest {
                item: it,
                ctx: args.ctx,
                policy: args.policy.clone(),
                sink: args.sink,
                timestamp: args.timestamp,
            },
            kind_key_pair.0,
            kind_key_pair.1,
            args.signals,
            &mut |admitted: &MemoryItem| {
                // A hand-crafted bundle can carry a secret-shaped context that
                // never passed the export scrubber, and admission masks only
                // snapshot and gist for telemetry. Mask the admitted clone
                // before staging, exactly as the force-overwrite path does, so
                // no create path can persist an unscrubbed context.
                staged.push(BatchWrite::CreateItem {
                    item: masked_item(admitted),
                    actor: actor.clone(),
                });
                Ok(())
            },
        )?;
        if decision.pass {
            self.create += 1;
        } else {
            self.reject_reason(decision.rejection_reason.as_deref());
        }
        Ok(())
    }

    /// Gated create for a deferred carrier: the admitted (score-stamped) clone
    /// is held for the row executor; no `CreateItem` is staged (the executor
    /// inserts the row exactly once). Rejections mark the carrier unusable.
    fn gate_carrier(&mut self, it: &MemoryItem) -> Result<(), AmError> {
        let args = self.args;
        let (kind, key) = kind_key(it);
        let admitted_carriers = &mut self.admitted_carriers;
        let decision = gated_create_raw(
            &GateRequest {
                item: it,
                ctx: args.ctx,
                policy: args.policy.clone(),
                sink: args.sink,
                timestamp: args.timestamp,
            },
            kind,
            key,
            args.signals,
            &mut |admitted: &MemoryItem| {
                admitted_carriers.insert(it.id.clone(), admitted.clone());
                Ok(())
            },
        )?;
        if decision.pass {
            self.create += 1;
        } else {
            self.rejected_carriers.insert(
                it.id.clone(),
                decision
                    .rejection_reason
                    .clone()
                    .unwrap_or_else(|| "admission gate rejection".to_owned()),
            );
            self.reject_reason(decision.rejection_reason.as_deref());
        }
        Ok(())
    }

    fn plan_opaque(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        let source_bank = if self.envelope.manifest.bank_ids.iter().any(|b| b == bank) {
            bank
        } else {
            self.envelope
                .manifest
                .bank_ids
                .first()
                .map_or("", String::as_str)
        };
        if !self.envelope.entities.opaque_items.is_empty() {
            self.note(
                "ciphertext_backup items restore content opaque under the local KMS; \
                 decrypted restore needs shared key custody (out of first release)"
                    .to_owned(),
            );
        }
        for opaque in &self.envelope.entities.opaque_items {
            let mut record = opaque_record(opaque, bank);
            record.id = import_id(bank, source_bank, &opaque.id).into_owned();
            if args.store.get_item_identity(bank, &record.id)?.is_some() {
                if args.force {
                    self.overwrite += 1;
                    let record_id = record.id.clone();
                    self.staged.push(BatchWrite::PutOpaqueItem {
                        item: record,
                        actor: actor.to_owned(),
                    });
                    self.stage_force_audit(bank, &record_id, actor);
                } else {
                    self.skip += 1;
                }
            } else {
                self.staged.push(BatchWrite::PutOpaqueItem {
                    item: record,
                    actor: actor.to_owned(),
                });
                self.create += 1;
            }
        }
        Ok(())
    }

    fn plan_assoc(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        for edge in &self.envelope.entities.assoc_edges {
            let mut e = edge.clone();
            bank.clone_into(&mut e.bank_id);
            e.id = import_id(bank, &edge.bank_id, &edge.id).into_owned();
            if self.bundle_item(&edge.source_id).is_some() {
                e.source_id = import_id(bank, &edge.bank_id, &edge.source_id).into_owned();
            }
            if self.bundle_item(&edge.target_id).is_some() {
                e.target_id = import_id(bank, &edge.bank_id, &edge.target_id).into_owned();
            }
            if args.store.get_assoc_edge(bank, &e.id)?.is_some() {
                if args.force {
                    self.overwrite += 1;
                    let edge_id = e.id.clone();
                    self.staged.push(BatchWrite::PutAssocEdge {
                        edge: e,
                        actor: actor.to_owned(),
                    });
                    self.stage_force_audit(bank, &edge_id, actor);
                } else {
                    self.skip += 1;
                }
            } else {
                self.staged.push(BatchWrite::PutAssocEdge {
                    edge: e,
                    actor: actor.to_owned(),
                });
                self.create += 1;
            }
        }
        Ok(())
    }

    fn plan_tombstones(&mut self) {
        let count = self.envelope.entities.tombstones.len();
        if count == 0 {
            return;
        }
        self.skip += count;
        self.note("tombstones are export provenance; not restored".to_owned());
    }
}

/// `items.id`-style primary keys are deployment-unique, so a cross-bank
/// restore writes a derived id; derived from (source id, target bank) only, so
/// re-importing the same bundle into the same bank stays idempotent.
pub(super) fn import_id<'i>(bank: &str, source_bank: &str, id: &'i str) -> Cow<'i, str> {
    if bank == source_bank {
        Cow::Borrowed(id)
    } else {
        Cow::Owned(format!("{id}#{bank}"))
    }
}

/// Candidate discriminator strings for the gated create path.
pub(super) fn kind_key(item: &MemoryItem) -> (&'static str, &'static str) {
    match item.kind {
        ItemKind::Semantic => (
            "semantic",
            item.category.map_or("", SemanticCategory::as_str),
        ),
        ItemKind::Episodic => (
            "episodic",
            item.episodic_type
                .map_or("", clio_types::EpisodicType::as_str),
        ),
    }
}

/// Defensive masking pass before a force overwrite (diagnostics-grade masking
/// on top of the export-time masking already present in the bundle). A context
/// that scrubs to an empty string is treated as absent, never written as `Some("")`.
fn masked_item(item: &MemoryItem) -> MemoryItem {
    let mut out = item.clone();
    if let Some(snapshot) = out.snapshot.as_mut() {
        *snapshot = masked_clone(snapshot);
    }
    if let Some(gist) = out.gist.as_mut() {
        *gist = scrub_inline_secrets(gist);
    }
    out.context = scrub_context(out.context.as_deref());
    out
}

#[cfg(test)]
#[path = "import_apply_tests.rs"]
mod import_apply_tests;
