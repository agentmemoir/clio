// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for planning the restore of `CiphertextBackup`
//! opaque items: the rows whose content stays sealed under the exporting
//! node's key custody.
//!
//! ## Owns
//! - The same-bank admission check for opaque rows and its fail-closed error
//! - Opaque create / skip / overwrite staging
//!
//! ## Does not own
//! - The shared planner state (`import_apply.rs`)
//! - Item, assoc, triple, task, failure, belief, or persona planning
//! - Store SQL or admission math
//!
//! ## Boundary
//!
//! Changes to opaque restore semantics should normally require changes here.
//! Changes to the shared planner state should not.
//!
//! Keep this module focused on opaque item import planning.

use clio_store::BatchWrite;
use clio_types::{AmError, ErrorCode};

use super::Planner;
use crate::import::import_rows::opaque_record;

impl Planner<'_> {
    pub(super) fn plan_opaque(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        // `OpaqueItem` carries no bank provenance, so "same bank?" is provable
        // only when the manifest names exactly the target bank — what every
        // exporter emits. `CiphertextBackup` rows hold the exporting node's
        // at-rest bytes, and import has no plaintext or DEK handle to re-seal
        // them, so any other manifest cannot produce a row that opens here.
        let same_bank = {
            let bank_ids = &self.envelope.manifest.bank_ids;
            bank_ids.len() == 1 && bank_ids[0] == bank
        };
        if !same_bank && let Some(opaque) = self.envelope.entities.opaque_items.first() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "cannot restore opaque item `{}` into bank `{bank}`: the bundle \
                     names a different bank, and its ciphertext cannot open here",
                    opaque.id
                ),
            ));
        }
        if !self.envelope.entities.opaque_items.is_empty() {
            self.note(
                "ciphertext_backup items restore content opaque under the local KMS; \
                 decrypted restore needs shared key custody (out of first release)"
                    .to_owned(),
            );
        }
        for opaque in &self.envelope.entities.opaque_items {
            let mut record = opaque_record(opaque, bank);
            record.id.clone_from(&opaque.id);
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
}

#[cfg(test)]
#[path = "import_apply_opaque_tests.rs"]
mod import_apply_opaque_tests;
