// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `compose_context`: packing the always-on
//! persona channel plus selected memories into one bounded injection pack.
//!
//! ## Owns
//! - `Composer`, `ComposeRequest`, `ComposePack`, `PackSection`, `PersonaEntry`
//! - Dual-budget accounting (persona vs memory) and ranked truncation
//! - The pack fidelity channel: it becomes `include_exact` on the nested
//!   retrieval request, and it is the channel the pack selector is called with
//!
//! ## Does not own
//! - Persona document storage/ranking (clio-persona fills the entries later)
//! - Retrieval ranking (`hybrid`) or the intent gate (`intent`)
//!
//! ## Boundary
//!
//! Changes to pack budgets or section framing should normally require changes
//! here. Changes to retrieval ranking should not require changes here.
//!
//! # Budget contract
//!
//! The persona section is always emitted and capped by its own budget; memory
//! is filled from retrieval under the remaining budget. Total tokens never
//! exceed `budget_tokens`, so a large bank can never force a larger injection.
//! Explicit `compose_context` runs retrieval whenever a query is present even
//! if an earlier `intent_gate` said skip; the automatic path (gate supplied and
//! skipping) omits memory but still emits the persona section.
//!
//! Keep this module focused on the bounded context pack.

use crate::dedup::DedupReport;
use crate::floors::MinScores;
use crate::hybrid::HybridRetriever;
use crate::intent::GateDecision;
use crate::pack_forms::{FormTally, HitForm, hit_form_text};
use crate::token::estimate_tokens;
use crate::types::{RetrieveRequest, ScoredHit};
use clio_config::retention::{DuplicateTolerance, RecallScope};
use clio_config::retention_levels::{PackFidelityDefault, ProvenanceFloorDefault};
use clio_types::{AmError, ErrorCode, TimeAxis};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::sync::Arc;

/// Default total injection budget when a caller omits one.
pub const DEFAULT_BUDGET_TOKENS: usize = 2048;
/// Default persona-channel budget target.
pub const DEFAULT_PERSONA_BUDGET_TOKENS: usize = 400;
/// Section framing overhead counted once per pack.
pub const PACK_OVERHEAD_TOKENS: usize = 8;
/// Default memory fetch size before budget truncation.
pub const DEFAULT_MEMORY_FETCH: usize = 50;

/// One pre-rendered persona entry supplied by the persona layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaEntry {
    /// Stable entry key (ranked tie-break).
    pub key: String,
    /// Rendered entry text.
    pub text: String,
    /// Admission score used for ranked truncation.
    #[serde(default)]
    pub score: f64,
}

/// Arguments for one compose call.
#[derive(Debug, Clone)]
pub struct ComposeRequest {
    /// Bank isolation key.
    pub bank: String,
    /// Query for the memory section (`None` = persona only).
    pub query: Option<String>,
    /// Domain filters for the memory section.
    pub domains: Vec<String>,
    /// Total token budget.
    pub budget_tokens: usize,
    /// Pre-rendered persona entries (empty stub allowed until the persona layer fills it).
    pub persona_entries: Vec<PersonaEntry>,
    /// Point-in-time filter for the memory section.
    pub as_of: Option<String>,
    /// Axis for `as_of`.
    pub time_axis: TimeAxis,
    /// Bounded graph expansion for the memory section.
    pub expand_graph: bool,
    /// Injectable clock for deterministic recency boosts.
    pub now: Option<String>,
    /// Explicit anchor for a relative temporal expression in `query` (RFC 3339
    /// with an explicit UTC offset). `None` means the temporal arm does not run
    /// for the memory section: there is no server-time fallback.
    pub query_anchor: Option<String>,
    /// Recall scope preset for the memory section (bank default already resolved).
    pub recall_scope: RecallScope,
    /// Drop raw items whose consolidated parent is also returned in the memory section.
    pub prefer_consolidated: bool,
    /// Per-bank read-time tolerance level for the memory section.
    pub duplicate_tolerance_read: DuplicateTolerance,
    /// Read-time retention curve active flag for the memory section (per-bank
    /// policy). False keeps the pre-curve order.
    pub retention_curve: bool,
    /// Curve floor in `(0, 1]`, read only when `retention_curve` is true.
    pub retention_floor: f64,
    /// Provenance floor for the memory section, resolved against the bank
    /// default by the caller. `None` means no floor.
    pub provenance_floor: Option<ProvenanceFloorDefault>,
    /// Fidelity channel for the memory section's per-hit form (bank default
    /// already resolved). `Flow` reads each hit's gist first, `Exact` reads
    /// its bounded rendering first. It is a fidelity choice only: it never
    /// changes which hits are returned or their order.
    pub fidelity: PackFidelityDefault,
}

impl ComposeRequest {
    /// Minimal request for one bank with the default budget and scope.
    #[must_use]
    pub fn new(bank: impl Into<String>) -> Self {
        Self {
            bank: bank.into(),
            query: None,
            domains: Vec::new(),
            budget_tokens: DEFAULT_BUDGET_TOKENS,
            persona_entries: Vec::new(),
            as_of: None,
            time_axis: TimeAxis::Valid,
            expand_graph: false,
            now: None,
            query_anchor: None,
            recall_scope: RecallScope::Full,
            prefer_consolidated: false,
            duplicate_tolerance_read: DuplicateTolerance::Balanced,
            retention_curve: false,
            retention_floor: 0.25,
            provenance_floor: None,
            fidelity: PackFidelityDefault::Flow,
        }
    }
}

/// One packed section of the injection pack.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PackSection {
    /// Section name (`persona` | `memory`).
    pub name: String,
    /// Estimated tokens in this section.
    pub tokens: usize,
    /// Item ids included (persona keys or memory item ids).
    pub item_ids: Vec<String>,
    /// Form each included memory hit took, parallel to `item_ids` (same
    /// order, included hits only). Empty for the persona section, which
    /// never fills it, so its serialization is unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hit_forms: Vec<HitForm>,
    /// Rendered section text.
    pub text: String,
    /// True when at least one candidate was dropped for budget.
    pub truncated: bool,
}

impl PackSection {
    fn empty(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            tokens: 0,
            item_ids: Vec::new(),
            hit_forms: Vec::new(),
            text: String::new(),
            truncated: false,
        }
    }
}

/// The bounded pack the system would inject for one turn.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComposePack {
    /// Persona section first, then memory.
    pub sections: Vec<PackSection>,
    /// Estimated total tokens (≤ `budget_tokens`).
    pub total_tokens: usize,
    /// Caller budget.
    pub budget_tokens: usize,
    /// Persona budget actually available.
    pub persona_budget_tokens: usize,
    /// Memory budget actually available.
    pub memory_budget_tokens: usize,
    /// Candidate ids dropped for budget.
    pub truncated_ids: Vec<String>,
    /// Whether the memory section ran retrieval.
    pub retrieval_ran: bool,
    /// Structured, PII-safe warnings.
    pub warnings: Vec<String>,
    /// Scope/dedup counters for the memory section; omitted when nothing was narrowed.
    #[serde(skip_serializing_if = "DedupReport::is_inert")]
    pub dedup: DedupReport,
    /// How many included memory hits contributed each form; folded once from
    /// the memory section's record. Always serialized: a zero tally is a fact.
    pub form_tally: FormTally,
}

/// Context composer over a [`HybridRetriever`].
pub struct Composer {
    retriever: Arc<HybridRetriever>,
    persona_budget_tokens: usize,
}

impl std::fmt::Debug for Composer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composer")
            .field("persona_budget_tokens", &self.persona_budget_tokens)
            .finish_non_exhaustive()
    }
}

impl Composer {
    /// Build a composer with the default persona budget.
    #[must_use]
    pub fn new(retriever: Arc<HybridRetriever>) -> Self {
        Self {
            retriever,
            persona_budget_tokens: DEFAULT_PERSONA_BUDGET_TOKENS,
        }
    }

    /// Override the persona-channel budget.
    #[must_use]
    pub fn with_persona_budget(mut self, tokens: usize) -> Self {
        self.persona_budget_tokens = tokens;
        self
    }

    /// Compose a bounded pack.
    ///
    /// `gate` is the automatic-path decision: when supplied and skipping,
    /// memory is omitted but the persona section is still emitted. Explicit
    /// callers pass `None` so retrieval always runs when a query is present.
    ///
    /// # Errors
    /// Budget underflow, missing bank, or retrieval failures.
    pub fn compose(
        &self,
        req: &ComposeRequest,
        gate: Option<&GateDecision>,
    ) -> Result<ComposePack, AmError> {
        if req.budget_tokens <= PACK_OVERHEAD_TOKENS {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "compose budget_tokens must exceed the {PACK_OVERHEAD_TOKENS}-token pack overhead"
                ),
            ));
        }
        if req.bank.is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "compose requires a non-empty bank",
            ));
        }
        let mut warnings = Vec::new();
        let mut dedup = DedupReport::default();
        let persona_budget = self
            .persona_budget_tokens
            .min(req.budget_tokens - PACK_OVERHEAD_TOKENS);
        let persona = pack_persona(&req.persona_entries, persona_budget);
        let memory_budget = req.budget_tokens - PACK_OVERHEAD_TOKENS - persona.tokens;
        let auto_skip = matches!(gate, Some(g) if !g.retrieval_needed);
        let (memory, retrieval_ran) = if let Some(query) = &req.query {
            if auto_skip {
                (PackedSection::empty("memory"), false)
            } else {
                let outcome = self.retriever.retrieve(&RetrieveRequest {
                    bank: req.bank.clone(),
                    query: query.clone(),
                    domains: req.domains.clone(),
                    limit: DEFAULT_MEMORY_FETCH,
                    as_of: req.as_of.clone(),
                    time_axis: req.time_axis,
                    expand_graph: req.expand_graph,
                    budget_tokens: Some(memory_budget),
                    now: req.now.clone(),
                    query_anchor: req.query_anchor.clone(),
                    recall_scope: req.recall_scope,
                    prefer_consolidated: req.prefer_consolidated,
                    duplicate_tolerance_read: req.duplicate_tolerance_read,
                    retention_curve: req.retention_curve,
                    retention_floor: req.retention_floor,
                    // The bank's provenance floor is a read policy, so the pack
                    // honours it; only the score floors stay unset, because a
                    // composed pack exposes no scores to filter on.
                    provenance_floor: req.provenance_floor,
                    // The pack is text, so it exposes no per-hit scores and no
                    // entity names: a composed pack keeps the retrieval defaults
                    // (no floor, entities projected per hit, no summary).
                    min_scores: MinScores::default(),
                    include_entities: true,
                    entity_summary: false,
                    // The channel, not the caller, asks for the rendering: the
                    // pack reads it only where it would win the form, so a flow
                    // request produces none.
                    include_exact: req.fidelity == PackFidelityDefault::Exact,
                })?;
                warnings.extend(outcome.warnings.clone());
                dedup = outcome.dedup.clone();
                (
                    pack_memory(&outcome.hits, memory_budget, req.fidelity),
                    true,
                )
            }
        } else {
            (PackedSection::empty("memory"), false)
        };
        let total_tokens = PACK_OVERHEAD_TOKENS + persona.tokens + memory.tokens;
        // One fold, one source: the tally comes only from the memory
        // section's per-hit record, so a truncated hit can never be counted.
        let form_tally = FormTally::fold(memory.section.hit_forms.iter().copied());
        let truncated_ids: Vec<String> = persona
            .truncated_ids
            .iter()
            .chain(memory.truncated_ids.iter())
            .cloned()
            .collect();
        Ok(ComposePack {
            sections: vec![persona.section, memory.section],
            total_tokens,
            budget_tokens: req.budget_tokens,
            persona_budget_tokens: persona_budget,
            memory_budget_tokens: memory_budget,
            truncated_ids,
            retrieval_ran,
            warnings,
            dedup,
            form_tally,
        })
    }
}

struct PackedSection {
    section: PackSection,
    tokens: usize,
    truncated_ids: Vec<String>,
}

impl PackedSection {
    fn empty(name: &str) -> Self {
        Self {
            section: PackSection::empty(name),
            tokens: 0,
            truncated_ids: Vec::new(),
        }
    }
}

fn pack_persona(entries: &[PersonaEntry], budget: usize) -> PackedSection {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
    let mut ids = Vec::new();
    let mut truncated = Vec::new();
    let mut parts = Vec::new();
    let mut used = 0_usize;
    for entry in sorted {
        let tokens = estimate_tokens(&entry.key) + estimate_tokens(&entry.text);
        if used + tokens > budget {
            truncated.push(entry.key);
            continue;
        }
        used += tokens;
        ids.push(entry.key.clone());
        parts.push(format!("{}: {}", entry.key, entry.text));
    }
    PackedSection {
        section: PackSection {
            name: "persona".to_owned(),
            tokens: used,
            item_ids: ids,
            hit_forms: Vec::new(),
            text: parts.join("\n"),
            truncated: !truncated.is_empty(),
        },
        tokens: used,
        truncated_ids: truncated,
    }
}

fn pack_memory(hits: &[ScoredHit], budget: usize, channel: PackFidelityDefault) -> PackedSection {
    let mut ids = Vec::new();
    let mut forms = Vec::new();
    let mut truncated = Vec::new();
    let mut parts = Vec::new();
    let mut used = 0_usize;
    for hit in hits {
        // The total selector picks the form and the text, and it also owns the
        // budget fallback, so the form recorded here is the form of the text
        // emitted here. Every text, a rendering included, is charged through
        // the same estimator.
        let (form, text) = hit_form_text(
            channel,
            budget.saturating_sub(used),
            hit.hit.gist.as_deref(),
            hit.hit.exact_render.as_deref(),
            hit.snapshot_ref.as_deref(),
            &hit.hit.item_id,
        );
        let tokens = estimate_tokens(&text);
        if used + tokens > budget {
            truncated.push(hit.hit.item_id.clone());
            continue;
        }
        used += tokens;
        ids.push(hit.hit.item_id.clone());
        forms.push(form);
        parts.push(text);
    }
    PackedSection {
        section: PackSection {
            name: "memory".to_owned(),
            tokens: used,
            item_ids: ids,
            hit_forms: forms,
            text: parts.join("\n"),
            truncated: !truncated.is_empty(),
        },
        tokens: used,
        truncated_ids: truncated,
    }
}

#[cfg(test)]
#[path = "compose_exact_tests.rs"]
mod compose_exact_tests;

#[cfg(test)]
#[path = "compose_tests.rs"]
mod compose_tests;

#[cfg(test)]
#[path = "compose_hit_entities_tests.rs"]
mod compose_hit_entities_tests;
