// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP runtime signal sources and telemetry
//! sinks: the store-backed neighbor signal source, whose recency stays neutral,
//! the empty signal source, and the stderr event sink set that keeps telemetry
//! off the JSON-RPC stdout stream.
//!
//! ## Owns
//! - `StoreSignals` (store-backed `SignalSource` / `BeliefSignalSource`)
//! - `BatchSignals` (stored neighbors plus the items staged earlier in a batch)
//! - `NoSignals` (neutral source for paths that must not re-score)
//! - `StderrSinks` implementing every domain `*Sink` trait
//!
//! ## Does not own
//! - Runtime store/engine wiring (`runtime`)
//! - Domain event shapes (owned by each domain crate)
//!
//! ## Boundary
//!
//! Changes to signal-source or sink wiring should normally require changes here.
//! Changes to `McpState` construction should not require changes here.
//!
//! Keep this module focused on runtime signal sources and telemetry sinks.

use clio_admission::AdmissionEvent;
use clio_admission::EventSink;
use clio_admission::gated::{SignalSource, candidate_text};
use clio_belief::observe::{BeliefEvent, BeliefSink};
use clio_belief::signals::BeliefSignalSource;
use clio_history::{HistoryEvent, HistorySink};
use clio_persona::{PersonaEvent, PersonaSink};
use clio_store::{IndexStore, Store};
use clio_types::{AmError, MemoryItem};
use clio_write::telemetry::{VerifyEvent, VerifySink};
use clio_write::triple::{TripleEvent, TripleSink};
use clio_write::update::{ContinuousEvent, ContinuousSink};
use clio_write::{DiscardEvent, DiscardSink};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Newest items scanned per scoring call (work bound; never the whole bank).
const SIGNALS_RECENT_WINDOW: usize = 64;
/// Lexical-index hits fetched per scoring call.
const SIGNALS_LEXICAL_LIMIT: usize = 64;
/// Candidate-text characters used for the lexical query (FTS OR-term bound).
const SIGNALS_QUERY_CHARS: usize = 512;

/// Neutral signals: no neighbors.
///
/// This is **not** the gated write source. Every gated write path that scores
/// against stored memory uses [`StoreSignals`]. `NoSignals` remains for tests
/// and for hosts that intentionally skip store scoring; no shipped write tool
/// uses it.
#[derive(Debug, Default)]
pub struct NoSignals;

impl SignalSource for NoSignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

impl BeliefSignalSource for NoSignals {
    fn neighbor_texts(&self, _bank_id: &str, _proposition: &str) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

/// Store-backed neighbor signals for the gated write path.
///
/// Used by every gated write surface whose rows live in `items`: `store`
/// (`remember`/`admit`/`admit --file`), `canonical_put`, `triple_add`,
/// `task_upsert`, `failure_record`, `belief_observe` create, raw ingest, and
/// plaintext portability `import` (the same gates as `store`). Persona
/// stable/preference writes target other tables and stay neutral.
///
/// # Work bound
///
/// One lookup scans at most [`SIGNALS_RECENT_WINDOW`] of the newest items in the
/// bank, plus at most [`SIGNALS_LEXICAL_LIMIT`] lexical-index hits. The bank is
/// never swept in full and the work does not grow with bank size, so a large
/// bank cannot make a write quadratic. The recent window catches just-written
/// items the async indexer has not published yet; the lexical leg catches older
/// indexed matches the window misses.
///
/// # Novelty
///
/// Neighbor texts are the [`candidate_text`] of the scanned items (self
/// excluded), so an exact duplicate in the window scores novelty `0.0` and the
/// duplicate gate can fire.
pub struct StoreSignals {
    store: Arc<dyn Store>,
    index: Arc<dyn IndexStore>,
    recent_window: usize,
    lexical_limit: usize,
}

impl std::fmt::Debug for StoreSignals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreSignals").finish_non_exhaustive()
    }
}

impl StoreSignals {
    /// Bind the signal source to one backend's store and index views.
    #[must_use]
    pub fn new(store: Arc<dyn Store>, index: Arc<dyn IndexStore>) -> Self {
        Self {
            store,
            index,
            recent_window: SIGNALS_RECENT_WINDOW,
            lexical_limit: SIGNALS_LEXICAL_LIMIT,
        }
    }

    /// Bind with explicit work bounds (tests and hosts that tune the budget).
    #[must_use]
    pub fn with_limits(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        recent_window: usize,
        lexical_limit: usize,
    ) -> Self {
        Self {
            store,
            index,
            recent_window: recent_window.max(1),
            lexical_limit,
        }
    }

    /// The newest `recent_window` items in `bank`, ascending by creation time.
    ///
    /// `count_indexable_items` is a cheap aggregate; the page then fetches only
    /// the trailing window, so the number of decrypted rows is bounded.
    fn recent_items(&self, bank: &str) -> Result<Vec<MemoryItem>, AmError> {
        let total = usize::try_from(self.index.count_indexable_items(bank)?).unwrap_or(usize::MAX);
        if total == 0 {
            return Ok(Vec::new());
        }
        let offset = total.saturating_sub(self.recent_window);
        let ids = self
            .index
            .indexable_items(bank, self.recent_window, offset)?;
        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(item) = self.readable_neighbor(bank, id)? {
                out.push(item);
            }
        }
        Ok(out)
    }

    /// Lexical-index neighbor ids for `text` (bounded), or empty when unavailable.
    fn indexed_neighbor_ids(&self, bank: &str, text: &str) -> Result<Vec<String>, AmError> {
        if text.trim().is_empty() || self.lexical_limit == 0 {
            return Ok(Vec::new());
        }
        // Bound the query length: the whole candidate text can carry a large
        // snapshot, and FTS turns it into an OR of every token.
        let query: String = text.chars().take(SIGNALS_QUERY_CHARS).collect();
        let hits = self
            .index
            .lexical_search(bank, &query, &[], self.lexical_limit)?;
        Ok(hits.into_iter().map(|hit| hit.item_id).collect())
    }

    /// Read one neighbor without letting damaged content abort a new write.
    ///
    /// A row whose content cannot be opened — damaged envelope, undecodable
    /// nonce or ciphertext, missing or foreign content format, no DEK, AEAD
    /// failure, crypto-shredded subject — is skipped, so one bad row costs only
    /// its own novelty contribution. Genuine backend failures (driver, database,
    /// key store) still fail closed, which is what keeps
    /// `factor_unavailable` meaningful. `clio_store::is_unreadable_content` is the
    /// single definition of that split.
    ///
    /// # What the skip costs
    ///
    /// A skipped neighbor contributes nothing to novelty, so the candidate looks
    /// *less* like a duplicate of it. A near-duplicate of a damaged row can
    /// therefore pass the duplicate gate. This is a deliberate trade, not an
    /// oversight: the alternative — standing in a neutral placeholder text so
    /// novelty still sees the row — would push the candidate *over* the floor and
    /// reject the write, which is the write-hostility this skip exists to remove.
    /// One damaged row must not make the bank unwritable. The exposure is
    /// bounded by `novelty_floor()`, whose documented default rejects nothing
    /// extra; deployments that raise `duplicate_tolerance_write` are where the
    /// trade becomes visible. See `clio_store::is_unreadable_content` and
    /// `unreadable_neighbor_lets_a_duplicate_of_a_damaged_row_through`.
    fn readable_neighbor(&self, bank: &str, item_id: &str) -> Result<Option<MemoryItem>, AmError> {
        match self.store.get_memory_item(bank, item_id) {
            Err(error) if clio_store::is_unreadable_content(&error) => Ok(None),
            result => result,
        }
    }

    /// Bounded neighbor set for one query: recent window plus indexed hits.
    ///
    /// Recent items are fetched once; an indexed hit is fetched only when the
    /// window did not already carry it, so the two legs never double-decrypt.
    /// Unreadable content is omitted from the signal set. `self_id` excludes
    /// the candidate's own stored row when it already exists (idempotent
    /// re-writes); `None` for a belief proposition, which has no row.
    fn bounded_neighbors_for(
        &self,
        bank: &str,
        query: &str,
        self_id: Option<&str>,
    ) -> Result<Vec<MemoryItem>, AmError> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        if let Some(id) = self_id {
            seen.insert(id.to_owned());
        }
        let mut items = Vec::new();
        for item in self.recent_items(bank)? {
            if seen.insert(item.id.clone()) {
                items.push(item);
            }
        }
        for id in self.indexed_neighbor_ids(bank, query)? {
            if seen.insert(id.clone())
                && let Some(item) = self.readable_neighbor(bank, &id)?
            {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Bounded neighbor set for one candidate item.
    fn bounded_neighbors(&self, item: &MemoryItem) -> Result<Vec<MemoryItem>, AmError> {
        self.bounded_neighbors_for(
            item.bank_id.as_str(),
            &candidate_text(item),
            Some(item.id.as_str()),
        )
    }
}

impl SignalSource for StoreSignals {
    fn neighbor_texts(&self, item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(self
            .bounded_neighbors(item)?
            .iter()
            .map(candidate_text)
            .collect())
    }
}

impl BeliefSignalSource for StoreSignals {
    fn neighbor_texts(&self, bank_id: &str, proposition: &str) -> Result<Vec<String>, AmError> {
        Ok(self
            .bounded_neighbors_for(bank_id, proposition, None)?
            .iter()
            .map(candidate_text)
            .collect())
    }
}

/// Store-backed neighbor signals plus the items already staged earlier in the
/// same `batch`, so a later operation is scored against the operations in front
/// of it before the batch transaction commits them.
///
/// Pre-flight scores each operation against committed rows only. Without the
/// staged texts, an operation that repeats an earlier operation's item text
/// slips past the exact-duplicate gate and both rows commit. The staged items
/// join the neighbor set on purpose, so the whole admission decision — the
/// exact-duplicate rejection and the novelty factor behind the score threshold —
/// sees the batch as it will exist after commit. Staged items are matched by
/// bank: a duplicate in another bank is not a duplicate for this item, and
/// `batch` may span several banks.
#[derive(Debug)]
pub struct BatchSignals<'a> {
    inner: StoreSignals,
    staged: &'a [MemoryItem],
}

impl<'a> BatchSignals<'a> {
    /// Bind the store-backed signals and the batch's already-staged items.
    #[must_use]
    pub fn new(
        store: Arc<dyn Store>,
        index: Arc<dyn IndexStore>,
        staged: &'a [MemoryItem],
    ) -> Self {
        Self {
            inner: StoreSignals::new(store, index),
            staged,
        }
    }
}

impl SignalSource for BatchSignals<'_> {
    fn neighbor_texts(&self, item: &MemoryItem) -> Result<Vec<String>, AmError> {
        let mut texts = SignalSource::neighbor_texts(&self.inner, item)?;
        texts.extend(
            self.staged
                .iter()
                .filter(|staged| staged.bank_id == item.bank_id)
                .map(candidate_text),
        );
        Ok(texts)
    }
}

/// Telemetry sink: writes serialized events to stderr (never stdout, so
/// stdio JSON-RPC framing stays pure).
#[derive(Debug, Default)]
pub struct StderrSinks;

fn log_event(operation: &str, value: &impl serde::Serialize) {
    let line = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    eprintln!("{operation} {line}");
}

impl EventSink for StderrSinks {
    fn emit(&self, event: AdmissionEvent) {
        log_event("admission", &event);
    }
}

impl VerifySink for StderrSinks {
    fn emit(&self, event: VerifyEvent) {
        log_event("verify", &event);
    }
}

impl TripleSink for StderrSinks {
    fn emit(&self, event: TripleEvent) {
        log_event("triple", &event);
    }
}

impl ContinuousSink for StderrSinks {
    fn emit(&self, event: ContinuousEvent) {
        log_event("continuous", &event);
    }
}

impl DiscardSink for StderrSinks {
    fn emit(&self, event: DiscardEvent) {
        log_event("discard", &event);
    }
}

impl clio_compliance::EraseSink for StderrSinks {
    fn emit(&self, event: clio_compliance::EraseEvent) {
        log_event("compliance_erase", &event);
    }
}

impl PersonaSink for StderrSinks {
    fn emit(&self, event: PersonaEvent) {
        log_event("persona", &event);
    }
}

impl HistorySink for StderrSinks {
    fn emit(&self, event: HistoryEvent) {
        log_event("history", &event);
    }
}

impl BeliefSink for StderrSinks {
    fn emit(&self, event: BeliefEvent) {
        log_event("belief", &event);
    }
}

#[cfg(test)]
#[path = "runtime_sinks_tests.rs"]
mod runtime_sinks_tests;

#[cfg(test)]
#[path = "runtime_sinks_duplicate_waiver_tests.rs"]
mod runtime_sinks_duplicate_waiver_tests;

#[cfg(test)]
#[path = "runtime_sinks_content_tests.rs"]
mod runtime_sinks_content_tests;
