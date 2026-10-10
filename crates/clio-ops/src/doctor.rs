// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `doctor` read-only planner: composing an
//! ordered, reviewable repair plan from verify findings, plus the process-local
//! plan registry that lets a later `repair` call resolve a returned plan id.
//!
//! ## Owns
//! - [`doctor`]: finding-to-action mapping, risk/quiesce annotations, and
//!   non-repairable notes
//! - [`RepairAction`], [`DoctorPlan`], and [`PlanRegistry`]
//! - The repair action allowlist (`known_actions` / `is_known_action`)
//!
//! ## Does not own
//! - Consistency checks (`verify`)
//! - Applying repairs (`repair`) or rebuilding indexes (`reindex`)
//!
//! ## Boundary
//!
//! Changes to plan composition or the action allowlist should normally require
//! changes here. Changes to check logic should not require changes here.
//!
//! `doctor` never mutates the store under any `dry_run` value: it reads, then
//! (when `dry_run=false`) records the plan in the in-memory registry only.

use crate::finding::{Findings, OpsFinding, Severity, codes};
use crate::verify::{VerifyArgs, VerifyReport};
use clio_store::{IndexStore, OpsStore};
use clio_types::AmError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Allowlisted repair action: drop association edges with missing endpoints.
pub const ACTION_DROP_ORPHAN_ASSOC_EDGE: &str = "drop_orphan_assoc_edge";
/// Allowlisted repair action: re-derive dense vectors for the scope.
pub const ACTION_REBUILD_DENSE_INDEX: &str = "rebuild_dense_index";
/// Allowlisted repair action: re-derive lexical docs for the scope.
pub const ACTION_REBUILD_LEXICAL_INDEX: &str = "rebuild_lexical_index";

/// Every repair action this build will execute.
#[must_use]
pub fn known_actions() -> &'static [&'static str] {
    &[
        ACTION_DROP_ORPHAN_ASSOC_EDGE,
        ACTION_REBUILD_DENSE_INDEX,
        ACTION_REBUILD_LEXICAL_INDEX,
    ]
}

/// True when `id` is in the repair allowlist.
#[must_use]
pub fn is_known_action(id: &str) -> bool {
    known_actions().contains(&id)
}

/// One ordered repair step with its rationale and risk notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairAction {
    /// Allowlisted action id.
    pub id: String,
    /// Finding codes this action addresses.
    pub rationale: Vec<String>,
    /// Risk band (`low` / `medium`).
    pub risk: String,
    /// True when writers should be quiesced during the action.
    pub quiesce_recommended: bool,
}

/// A composed, reviewable repair plan (`doctor_repair_plan`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorPlan {
    /// Registry id later passed to `repair`.
    pub plan_id: String,
    /// Composition timestamp.
    pub generated_at: String,
    /// Scope (`None` = whole store).
    pub bank: Option<String>,
    /// True for the default (print-only) plan.
    pub dry_run: bool,
    /// Verify aggregate `ok` at composition time.
    pub ok: bool,
    /// Ordered repair actions.
    pub actions: Vec<RepairAction>,
    /// Findings that motivated the plan.
    pub findings: Vec<OpsFinding>,
    /// Notes for non-repairable findings and skipped checks.
    pub notes: Vec<String>,
}

/// Process-local registry mapping a returned plan id to its plan.
///
/// Plan ids are session-scoped: a later `repair` in the same process can
/// resolve a plan id, while cross-process callers pass an explicit action list.
#[derive(Debug, Default)]
pub struct PlanRegistry {
    inner: Mutex<HashMap<String, DoctorPlan>>,
    counter: AtomicU64,
}

impl PlanRegistry {
    /// New empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a unique plan id.
    #[must_use]
    pub fn allocate_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        format!("plan-{n}")
    }

    /// Store a plan under its `plan_id`.
    pub fn insert(&self, plan: DoctorPlan) {
        let mut guard = self.inner.lock().expect("plan registry poisoned");
        guard.insert(plan.plan_id.clone(), plan);
    }

    /// Fetch a plan by id.
    #[must_use]
    pub fn get(&self, plan_id: &str) -> Option<DoctorPlan> {
        let guard = self.inner.lock().expect("plan registry poisoned");
        guard.get(plan_id).cloned()
    }

    /// Number of registered plans.
    #[must_use]
    pub fn len(&self) -> usize {
        let guard = self.inner.lock().expect("plan registry poisoned");
        guard.len()
    }

    /// True when no plan is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Inputs for [`doctor`].
pub struct DoctorArgs<'a> {
    /// Operator health repository.
    pub store: &'a dyn OpsStore,
    /// Index backend view.
    pub index: &'a dyn IndexStore,
    /// Bank scope (`None` = whole store).
    pub bank: Option<&'a str>,
    /// Default `true`: print the plan only, register nothing.
    pub dry_run: bool,
    /// Caller-supplied timestamp (determinism).
    pub timestamp: &'a str,
    /// Plan registry receiving the plan when `dry_run=false`.
    pub plans: &'a PlanRegistry,
}

/// Compose an ordered repair plan from verify findings. Never mutates the store.
///
/// When `dry_run=false` the plan is recorded in `args.plans` so a later
/// `repair(plan_id)` can resolve it; the store itself is untouched either way.
///
/// # Errors
/// Storage/index read failures (an unreadable store cannot be planned for).
pub fn doctor(args: &DoctorArgs<'_>) -> Result<DoctorPlan, AmError> {
    let report = crate::verify::verify(&VerifyArgs {
        store: args.store,
        index: args.index,
        bank: args.bank,
        timestamp: args.timestamp,
    })?;
    let (actions, notes) = compose(&report);
    let plan = DoctorPlan {
        plan_id: args.plans.allocate_id(),
        generated_at: args.timestamp.to_owned(),
        bank: args.bank.map(str::to_owned),
        dry_run: args.dry_run,
        ok: report.ok,
        actions,
        findings: report.findings,
        notes,
    };
    if !args.dry_run {
        args.plans.insert(plan.clone());
    }
    Ok(plan)
}

/// Map verify findings to ordered actions plus manual-repair notes.
fn compose(report: &VerifyReport) -> (Vec<RepairAction>, Vec<String>) {
    let mut actions = Vec::new();
    let mut notes = Vec::new();
    let mut seen = Findings::new();
    for finding in &report.findings {
        if seen.findings.iter().any(|f| f.code == finding.code) {
            continue;
        }
        seen.push(finding.clone());
        match finding.code.as_str() {
            codes::ORPHAN_ASSOC_EDGE => actions.push(RepairAction {
                id: ACTION_DROP_ORPHAN_ASSOC_EDGE.to_owned(),
                rationale: vec![finding.code.clone()],
                risk: "low".to_owned(),
                quiesce_recommended: false,
            }),
            codes::DENSE_COVERAGE_GAP => actions.push(RepairAction {
                id: ACTION_REBUILD_DENSE_INDEX.to_owned(),
                rationale: vec![finding.code.clone()],
                risk: "medium".to_owned(),
                quiesce_recommended: true,
            }),
            codes::LEXICAL_COVERAGE_GAP => actions.push(RepairAction {
                id: ACTION_REBUILD_LEXICAL_INDEX.to_owned(),
                rationale: vec![finding.code.clone()],
                risk: "low".to_owned(),
                quiesce_recommended: false,
            }),
            codes::DANGLING_TRIPLE_REF
            | codes::DANGLING_SNAPSHOT_REF
            | codes::DANGLING_GIST_REF
            | codes::ORPHAN_MEMTREE_LEAF => notes.push(format!(
                "{}: manual repair required (not auto-repairable)",
                finding.code
            )),
            codes::MANIFEST_UNAVAILABLE => notes.push(format!(
                "{}: manifest drift check skipped (no persisted manifest)",
                finding.code
            )),
            _ => {
                if finding.severity == Severity::Error {
                    notes.push(format!(
                        "{}: no automated repair action available",
                        finding.code
                    ));
                }
            }
        }
    }
    (actions, notes)
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod doctor_tests;
