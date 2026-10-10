// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `temporal_history`: one read façade that
//! resolves a target to an existing store's trajectory (temporal).
//!
//! ## Owns
//! - Target syntax parsing and dispatch to triple / preference / belief reads
//! - Point-in-time filtering semantics documented per source
//! - Context surfacing on the belief trajectory preference context
//!   rides the store's `ContinuousPoint`s and is present only from the point
//!   where it became effective (points before it carry none)
//!
//! ## Does not own
//! - Triple, continuous, or belief persistence (all in `clio-store`)
//! - Any second "temporal log" table (no such store is created)
//!
//! ## Boundary
//!
//! Changes to target syntax or dispatch should normally require changes here.
//! Changes to the underlying stores should not.
//!
//! # Target syntax (documented)
//!
//! - `triple:<subject>:<predicate>` → all versions of that SPO edge
//! - `preference:<key>` → EMA trend series (`continuous_series`)
//! - `belief:<id-or-proposition>` → confidence trajectory
//!
//! An unknown prefix is a structured `invalid_argument` error. The façade is
//! read-only; belief history stays append-only.

use crate::{HistoryEvent, HistorySink, require_nonempty};
use clio_store::{BeliefStore, ContinuousStore, HistoryStore};
use clio_types::{
    AmError, ConfidenceEntry, ContinuousPoint, ErrorCode, HistoryPolicy, TimeAxis, TripleRecord,
    proposition_key,
};

/// The resolved source family of a `temporal_history` target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalTargetKind {
    /// Discrete fact / bi-temporal triple.
    Triple,
    /// Continuous preference series.
    Preference,
    /// Belief confidence trajectory.
    Belief,
}

/// Inputs for `temporal_history`.
pub struct TemporalHistoryArgs<'a> {
    /// Triple trajectory repository.
    pub history: &'a dyn HistoryStore,
    /// Continuous preference repository.
    pub continuous: &'a dyn ContinuousStore,
    /// Belief repository.
    pub beliefs: &'a dyn BeliefStore,
    /// Bank isolation key.
    pub bank_id: &'a str,
    /// `triple:subject:predicate` | `preference:key` | `belief:id-or-proposition`.
    pub target: &'a str,
    /// Optional point-in-time filter.
    pub as_of: Option<&'a str>,
    /// Axis for the triple point-in-time filter (default valid).
    pub time_axis: TimeAxis,
    /// Requested row count; capped by the policy.
    pub limit: Option<usize>,
    /// Read-bound policy.
    pub policy: &'a HistoryPolicy,
    /// History telemetry sink.
    pub sink: &'a dyn HistorySink,
}

/// Trajectory returned by `temporal_history`.
#[derive(Debug, Clone, PartialEq)]
pub enum TemporalHistoryOutcome {
    /// Discrete fact edges for a subject+predicate, oldest transaction time first.
    Triple {
        /// SPO subject.
        subject: String,
        /// SPO predicate.
        predicate: String,
        /// Ordered edge versions (closed and open).
        entries: Vec<TripleRecord>,
        /// Echo of the point-in-time filter.
        as_of: Option<String>,
        /// Axis the point-in-time filter used.
        time_axis: TimeAxis,
    },
    /// Continuous preference points; each point carries the key's stored
    /// preference context only from the point at or after the context's
    /// effective time, so points before it was introduced carry no context. The
    /// store keeps one context per key, so this is not a per-observation history.
    Preference {
        /// Preference key.
        key: String,
        /// Ordered `(as_of, state, trend)` points.
        points: Vec<ContinuousPoint>,
        /// Echo of the point-in-time filter.
        as_of: Option<String>,
    },
    /// Belief confidence trajectory.
    Belief {
        /// Resolved belief id.
        belief_id: String,
        /// Belief proposition text.
        proposition: String,
        /// Optional bounded context of the belief; `None` when the
        /// belief was written without context, the sealed value is unreadable,
        /// or the read's `as_of` predates the context's effective time
        /// (`context_as_of`: the store stamp from which the current context is
        /// effective — `created_at` at introduction, the correction stamp after
        /// an authorized correction).
        context: Option<String>,
        /// Ordered confidence entries.
        entries: Vec<ConfidenceEntry>,
        /// Echo of the point-in-time filter.
        as_of: Option<String>,
    },
}

impl TemporalHistoryOutcome {
    /// The resolved source family.
    #[must_use]
    pub const fn kind(&self) -> TemporalTargetKind {
        match self {
            Self::Triple { .. } => TemporalTargetKind::Triple,
            Self::Preference { .. } => TemporalTargetKind::Preference,
            Self::Belief { .. } => TemporalTargetKind::Belief,
        }
    }

    /// Number of trajectory points/entries returned.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Triple { entries, .. } => entries.len(),
            Self::Preference { points, .. } => points.len(),
            Self::Belief { entries, .. } => entries.len(),
        }
    }

    /// True when the trajectory is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Read one trajectory from existing write-path data.
///
/// # Errors
/// Missing bank/target, malformed target, unknown target type, unknown belief,
/// zero limit, or store failures.
pub fn temporal_history(args: &TemporalHistoryArgs<'_>) -> Result<TemporalHistoryOutcome, AmError> {
    require_nonempty(args.bank_id, "bank_id")?;
    require_nonempty(args.target, "target")?;
    let limit = args.policy.resolve_limit(args.limit)?;
    let (kind, head, tail) = parse_target(args.target)?;
    let outcome = match kind {
        TemporalTargetKind::Triple => {
            // Point-in-time filtering happens in the store query, before LIMIT,
            // so a bounded trajectory keeps the version current at `as_of`.
            let entries = match args.as_of {
                Some(as_of) => args.history.triple_history_at(
                    args.bank_id,
                    &head,
                    &tail,
                    as_of,
                    args.time_axis,
                    limit,
                )?,
                None => args
                    .history
                    .triple_history(args.bank_id, &head, &tail, limit)?,
            };
            TemporalHistoryOutcome::Triple {
                subject: head,
                predicate: tail,
                entries,
                as_of: args.as_of.map(str::to_owned),
                time_axis: args.time_axis,
            }
        }
        TemporalTargetKind::Preference => {
            let points =
                args.continuous
                    .continuous_series(args.bank_id, &head, args.as_of, limit)?;
            TemporalHistoryOutcome::Preference {
                key: head,
                points,
                as_of: args.as_of.map(str::to_owned),
            }
        }
        TemporalTargetKind::Belief => {
            let belief = resolve_belief(args.beliefs, args.bank_id, &head)?;
            let entries = args
                .beliefs
                .belief_history(args.bank_id, &belief.id, args.as_of)?;
            // The belief context is effective from the store-derived `context_as_of`
            // stamp: `created_at` at introduction, the correction stamp after an
            // authorized correction. A point-in-time read before then must not
            // surface it. `as_of = None` keeps the current context. Honest
            // semantics: a read between the original introduction and a later
            // correction returns NO context — the prior value is unrecoverable
            // by design (forbids retaining it) and the future-corrected
            // context is never surfaced early. The store keeps one context per
            // belief (not a per-entry history).
            let context = match args.as_of {
                Some(as_of)
                    if as_of
                        < belief
                            .context_as_of
                            .as_deref()
                            .unwrap_or(belief.created_at.as_str()) =>
                {
                    None
                }
                _ => belief.context,
            };
            TemporalHistoryOutcome::Belief {
                belief_id: belief.id,
                proposition: belief.proposition,
                context,
                entries,
                as_of: args.as_of.map(str::to_owned),
            }
        }
    };
    args.sink.emit(HistoryEvent {
        operation: "temporal_history".into(),
        outcome: "returned".into(),
        bank: args.bank_id.into(),
        actor: String::new(),
        timestamp: String::new(),
        task_id: String::new(),
        record_id: String::new(),
        record_count: outcome.len(),
        admission_score: None,
        rejection_reason: None,
    });
    Ok(outcome)
}

/// Parse a target into `(kind, head, tail)`.
///
/// # Errors
/// Missing/unknown prefix or missing component.
pub(crate) fn parse_target(target: &str) -> Result<(TemporalTargetKind, String, String), AmError> {
    let (prefix, rest) = target
        .split_once(':')
        .ok_or_else(|| malformed(target, "expected `<kind>:<target>`"))?;
    let (head, tail) = match prefix {
        "triple" => {
            let (subject, predicate) = rest
                .split_once(':')
                .ok_or_else(|| malformed(target, "expected `triple:<subject>:<predicate>`"))?;
            (subject, predicate)
        }
        "preference" | "belief" => (rest, ""),
        other => {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "unknown temporal target type `{other}`; expected triple|preference|belief"
                ),
            ));
        }
    };
    if head.is_empty() {
        return Err(malformed(target, "target component is empty"));
    }
    let kind = match prefix {
        "triple" => TemporalTargetKind::Triple,
        "preference" => TemporalTargetKind::Preference,
        _ => TemporalTargetKind::Belief,
    };
    if kind == TemporalTargetKind::Triple && tail.is_empty() {
        return Err(malformed(target, "triple predicate is empty"));
    }
    Ok((kind, head.to_owned(), tail.to_owned()))
}

fn malformed(target: &str, detail: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!("invalid temporal target `{target}`: {detail}"),
    )
}

/// Resolve a belief by id, falling back to the normalized proposition key.
fn resolve_belief(
    beliefs: &dyn BeliefStore,
    bank_id: &str,
    id_or_proposition: &str,
) -> Result<clio_types::BeliefObject, AmError> {
    if let Some(belief) = beliefs.get_belief(bank_id, id_or_proposition)? {
        return Ok(belief);
    }
    beliefs
        .find_belief_by_proposition(bank_id, &proposition_key(id_or_proposition))?
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::NotFound,
                format!("belief not found: `{id_or_proposition}`"),
            )
        })
}

#[cfg(test)]
#[path = "temporal_tests.rs"]
mod temporal_tests;
