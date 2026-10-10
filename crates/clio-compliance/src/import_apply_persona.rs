// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the import planners for persona rows:
//! stable entry versions and continuous preference states, including the
//! import-side `context` scrub every staged persona row passes.
//!
//! ## Owns
//! - `plan_stable` / `plan_preferences` planning and their dedupe rules
//! - The persona staging `context` scrub: the same rule the export masking
//!   applies (`scrub_context`), so no staged persona row persists a raw,
//!   secret-bearing, or over-bound context
//!
//! ## Does not own
//! - Item/opaque/assoc planning (`import_apply.rs`)
//! - Triple/task/failure/belief planning (`import_rows.rs`)
//! - Store SQL or admission math
//!
//! ## Boundary
//!
//! Changes to persona restore semantics (versioning, dedupe, force overwrite)
//! should normally require changes here. Changes to the shared planner state
//! should not.
//!
//! Keep this module focused on persona row import planning.

use clio_store::BatchWrite;
use clio_types::AmError;

use super::Planner;
use crate::export_mask::scrub_context;

impl Planner<'_> {
    /// Plan the bundle's persona stable versions (see the module doc for the
    /// staging `context` scrub rule).
    pub(super) fn plan_stable(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        for entry in &self.envelope.entities.persona_stable {
            let mut en = entry.clone();
            bank.clone_into(&mut en.bank_id);
            // Every staged persona row — force and plain create alike —
            // passes the same `context` scrub the export masking applies.
            en.context = scrub_context(en.context.as_deref());
            let history = args.persona.stable_history(bank, &en.key)?;
            // Persona identity includes `context`: a row whose context
            // differs is not identical. Without force the stored context is
            // preserved (ordinary updates never replace context); with
            // force the row lands on the audited overwrite path below.
            let identical = history.iter().any(|h| {
                h.key == en.key
                    && h.value == en.value
                    && h.as_of == en.as_of
                    && h.context == en.context
            });
            if identical {
                self.skip += 1;
            } else if history.is_empty() || args.force {
                if history.is_empty() {
                    self.create += 1;
                } else {
                    // put_stable_entry closes the prior open version.
                    self.overwrite += 1;
                    self.stage_force_audit(bank, &format!("persona-stable:{}", en.key), actor);
                }
                self.staged.push(BatchWrite::PutStableEntry {
                    entry: en,
                    actor: actor.to_owned(),
                });
            } else {
                self.skip += 1;
            }
        }
        Ok(())
    }

    #[allow(clippy::float_cmp)] // identity dedupe: exact stored state, bit-for-bit
    pub(super) fn plan_preferences(&mut self) -> Result<(), AmError> {
        let args = self.args;
        let bank = args.ctx.bank.as_str();
        let actor = args.ctx.actor.as_str();
        for pref in &self.envelope.entities.persona_preferences {
            let mut p = pref.clone();
            bank.clone_into(&mut p.bank_id);
            // Same scrub as the stable path, on every staged branch.
            p.context = scrub_context(p.context.as_deref());
            match args.persona.get_preference(bank, &p.key)? {
                None => {
                    self.staged.push(BatchWrite::PutPreferenceState {
                        preference: p,
                        actor: actor.to_owned(),
                    });
                    self.create += 1;
                }
                Some(existing)
                    if existing.value == p.value
                        && existing.trend == p.trend
                        && existing.as_of == p.as_of
                        && existing.context == p.context =>
                {
                    self.skip += 1;
                }
                Some(_) if args.force => {
                    let handle = format!("persona-preference:{}", p.key);
                    self.staged.push(BatchWrite::PutPreferenceState {
                        preference: p,
                        actor: actor.to_owned(),
                    });
                    self.overwrite += 1;
                    self.stage_force_audit(bank, &handle, actor);
                }
                Some(_) => {
                    self.skip += 1;
                }
            }
        }
        Ok(())
    }
}
