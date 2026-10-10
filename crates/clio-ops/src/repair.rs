// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `repair` operator surface: confirmation-
//! gated execution of allowlisted doctor plan actions with per-action outcomes
//! and targeted re-verification.
//!
//! ## Owns
//! - [`repair`]: plan/action resolution, confirmation gate, allowlist refusal,
//!   action execution, and residual finding re-verification
//! - [`ActionOutcome`] / [`RepairOutcome`] shapes
//!
//! ## Does not own
//! - The repair allowlist or plan composition (`doctor`)
//! - The rebuild sweep (`reindex` / `clio_index::rebuild`)
//!
//! ## Boundary
//!
//! Changes to repair execution policy should normally require changes here.
//! Changes to the allowlist should not (edit `doctor`).
//!
//! `repair` never touches compliance erasure or DEK destruction, and never runs
//! a caller-supplied SQL string: only the compiled allowlisted actions execute.

use crate::doctor::{
    ACTION_DROP_ORPHAN_ASSOC_EDGE, ACTION_REBUILD_DENSE_INDEX, ACTION_REBUILD_LEXICAL_INDEX,
    DoctorPlan, PlanRegistry, RepairAction,
};
use crate::finding::{Findings, OpsFinding, Severity, codes, scope_label};
use crate::reindex::{ReindexArgs, reindex};
use crate::verify::{VerifyArgs, verify};
use clio_index::{Embedder, RebuildTargets};
use clio_store::{IndexStore, OpsStore, Store};
use serde::Serialize;
use std::collections::BTreeSet;

/// Outcome of one attempted repair action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    /// The action applied and the targeted finding cleared.
    Repaired,
    /// The action had nothing to do (findings already clear).
    Skipped,
    /// The action could not be applied.
    Failed,
}

/// Per-action result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActionOutcome {
    /// Action id attempted.
    pub action: String,
    /// Outcome band.
    pub status: ActionStatus,
    /// PII-free explanation.
    pub detail: String,
    /// Rows affected (0 when skipped or failed).
    pub affected: u64,
}

/// Result of one `repair` call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RepairOutcome {
    /// True only when no refusal, no failed action, and no targeted residual.
    pub ok: bool,
    /// Confirmation was supplied.
    pub confirmed: bool,
    /// Plan id when one was resolved.
    pub plan_id: Option<String>,
    /// Scope repaired.
    pub bank: Option<String>,
    /// Per-action outcomes (empty when refused before execution).
    pub results: Vec<ActionOutcome>,
    /// Targeted findings still present after repair.
    pub residual_findings: Vec<OpsFinding>,
    /// Full verify `ok` after repair.
    pub residual_ok: bool,
    /// Refusal findings (empty on a confirmed run).
    pub findings: Vec<OpsFinding>,
}

/// Inputs for [`repair`].
pub struct RepairArgs<'a> {
    /// Operator health repository.
    pub store: &'a dyn OpsStore,
    /// Writer used by the dense rebuild sweep.
    pub writer: &'a dyn Store,
    /// Index backend view.
    pub index: &'a dyn IndexStore,
    /// Embedding sidecar (required for dense rebuild actions).
    pub embedder: Option<&'a dyn Embedder>,
    /// Plan registry for `plan_id` resolution.
    pub plans: &'a PlanRegistry,
    /// A doctor-returned plan id (mutually exclusive with `actions`).
    pub plan_id: Option<&'a str>,
    /// Explicit allowlisted action ids (used when no plan id is given).
    pub actions: &'a [String],
    /// Scope override (`None` = plan bank or whole store).
    pub bank: Option<&'a str>,
    /// Required `true` to mutate.
    pub confirm: bool,
    /// Caller-supplied timestamp (determinism).
    pub timestamp: &'a str,
}

/// Execute allowlisted repairs. Policy refusals return `Ok(..ok=false..)` with a
/// finding; storage failures return `Err`.
///
/// # Errors
/// Storage/index read failures while re-verifying or executing actions.
pub fn repair(args: &RepairArgs<'_>) -> Result<RepairOutcome, clio_types::AmError> {
    let Resolved {
        actions,
        plan,
        mut refusal,
    } = resolve_actions(args);
    if refusal.is_empty() {
        // Confirmation gate: refuse before any mutation when unconfirmed.
        if args.confirm {
            return execute_all(args, &actions, plan);
        }
        refusal.push(OpsFinding::new(
            codes::CONFIRM_REQUIRED,
            Severity::Error,
            scope_label(args.bank),
            "repair requires confirm=true to mutate".to_owned(),
        ));
    }
    Ok(refused(args, refusal, plan.as_ref()))
}

/// Resolved repair request: actions, resolved plan, and any refusal findings.
struct Resolved {
    actions: Vec<RepairAction>,
    plan: Option<DoctorPlan>,
    refusal: Findings,
}

/// Resolve a plan id or explicit action list against the allowlist.
fn resolve_actions(args: &RepairArgs<'_>) -> Resolved {
    let mut refusal = Findings::new();
    let mut plan = None;
    let actions = if let Some(plan_id) = args.plan_id {
        if let Some(found) = args.plans.get(plan_id) {
            plan = Some(found.clone());
            found.actions
        } else {
            refusal.push(OpsFinding::new(
                codes::PLAN_NOT_FOUND,
                Severity::Error,
                scope_label(args.bank),
                format!("plan `{plan_id}` not found; re-run doctor"),
            ));
            Vec::new()
        }
    } else if args.actions.is_empty() {
        refusal.push(OpsFinding::new(
            codes::UNKNOWN_REPAIR_ACTION,
            Severity::Error,
            scope_label(args.bank),
            "repair requires a plan_id or a non-empty actions list".to_owned(),
        ));
        Vec::new()
    } else {
        let unknown: Vec<String> = args
            .actions
            .iter()
            .filter(|id| !crate::doctor::is_known_action(id))
            .cloned()
            .collect();
        if unknown.is_empty() {
            args.actions
                .iter()
                .map(|id| RepairAction {
                    id: id.clone(),
                    rationale: Vec::new(),
                    risk: "low".to_owned(),
                    quiesce_recommended: false,
                })
                .collect()
        } else {
            refusal.push(OpsFinding::new(
                codes::UNKNOWN_REPAIR_ACTION,
                Severity::Error,
                scope_label(args.bank),
                format!("unknown repair action(s): {}", unknown.join(", ")),
            ));
            Vec::new()
        }
    };
    Resolved {
        actions,
        plan,
        refusal,
    }
}

/// Apply every resolved action, then re-verify the addressed findings.
fn execute_all(
    args: &RepairArgs<'_>,
    actions: &[RepairAction],
    plan: Option<DoctorPlan>,
) -> Result<RepairOutcome, clio_types::AmError> {
    let bank = args
        .bank
        .map(str::to_owned)
        .or_else(|| plan.as_ref().and_then(|p| p.bank.clone()));
    let mut results = Vec::new();
    let mut addressed: BTreeSet<String> = BTreeSet::new();
    for action in actions {
        for code in &action.rationale {
            addressed.insert(code.clone());
        }
        results.push(execute(args, action, bank.as_deref()));
    }
    if plan.is_none() {
        // Explicit action lists carry no rationale: default the targeted set.
        for action in actions {
            if let Some(code) = addressed_code(&action.id) {
                addressed.insert(code.to_owned());
            }
        }
    }

    let report = verify(&VerifyArgs {
        store: args.store,
        index: args.index,
        bank: bank.as_deref(),
        timestamp: args.timestamp,
    })?;
    let residual: Vec<OpsFinding> = report
        .findings
        .iter()
        .filter(|f| addressed.contains(&f.code))
        .cloned()
        .collect();
    let all_applied = results.iter().all(|r| r.status != ActionStatus::Failed);
    let ok = all_applied && residual.is_empty();
    Ok(RepairOutcome {
        ok,
        confirmed: true,
        plan_id: plan.map(|p| p.plan_id),
        bank,
        results,
        residual_findings: residual,
        residual_ok: report.ok,
        findings: Vec::new(),
    })
}

/// Build a refusal outcome (no mutation performed).
fn refused(args: &RepairArgs<'_>, refusal: Findings, plan: Option<&DoctorPlan>) -> RepairOutcome {
    RepairOutcome {
        ok: false,
        confirmed: false,
        plan_id: plan.map(|p| p.plan_id.clone()),
        bank: args
            .bank
            .map(str::to_owned)
            .or_else(|| plan.and_then(|p| p.bank.clone())),
        results: Vec::new(),
        residual_findings: Vec::new(),
        residual_ok: false,
        findings: refusal.into_vec(),
    }
}

/// Execute one allowlisted action and map it to an outcome.
fn execute(args: &RepairArgs<'_>, action: &RepairAction, bank: Option<&str>) -> ActionOutcome {
    let Some(bank) = bank else {
        return failed(action, "action requires a bank scope");
    };
    match action.id.as_str() {
        ACTION_DROP_ORPHAN_ASSOC_EDGE => match args.store.drop_orphan_assoc_edges(bank) {
            Ok(0) => skipped(action, "no orphan association edges remain"),
            Ok(n) => repaired(action, n, format!("dropped {n} orphan association edge(s)")),
            Err(e) => failed(action, &format!("drop orphan edges failed: {e}")),
        },
        ACTION_REBUILD_DENSE_INDEX => rebuild(args, action, bank, RebuildTargets::Dense),
        ACTION_REBUILD_LEXICAL_INDEX => rebuild(args, action, bank, RebuildTargets::Lexical),
        other => failed(action, &format!("unknown action `{other}`")),
    }
}

fn rebuild(
    args: &RepairArgs<'_>,
    action: &RepairAction,
    bank: &str,
    target: RebuildTargets,
) -> ActionOutcome {
    if target == RebuildTargets::Dense && args.embedder.is_none() {
        return failed(
            action,
            "dense rebuild requires an embedding sidecar (set embed.url)",
        );
    }
    match reindex(&ReindexArgs {
        store: args.writer,
        index: args.index,
        embedder: args.embedder,
        bank: Some(bank),
        target,
        dry_run: false,
        confirm: true,
    }) {
        Ok(report) => {
            let applied = if target == RebuildTargets::Dense {
                report.dense_applied
            } else {
                report.lexical_applied
            };
            if applied == 0 {
                skipped(action, "no index rows needed rebuilding")
            } else {
                repaired(
                    action,
                    applied,
                    format!("re-derived {applied} index row(s)"),
                )
            }
        }
        Err(e) => failed(action, &format!("rebuild failed: {e}")),
    }
}

fn repaired(action: &RepairAction, affected: u64, detail: String) -> ActionOutcome {
    ActionOutcome {
        action: action.id.clone(),
        status: ActionStatus::Repaired,
        detail,
        affected,
    }
}

fn skipped(action: &RepairAction, detail: &str) -> ActionOutcome {
    ActionOutcome {
        action: action.id.clone(),
        status: ActionStatus::Skipped,
        detail: detail.to_owned(),
        affected: 0,
    }
}

fn failed(action: &RepairAction, detail: &str) -> ActionOutcome {
    ActionOutcome {
        action: action.id.clone(),
        status: ActionStatus::Failed,
        // Choke point for driver errors can embed credentialed URLs.
        detail: crate::finding::redact_credentials(detail),
        affected: 0,
    }
}

/// Finding code an action targets (for explicit action lists with no rationale).
fn addressed_code(action_id: &str) -> Option<&'static str> {
    match action_id {
        ACTION_DROP_ORPHAN_ASSOC_EDGE => Some(codes::ORPHAN_ASSOC_EDGE),
        ACTION_REBUILD_DENSE_INDEX => Some(codes::DENSE_COVERAGE_GAP),
        ACTION_REBUILD_LEXICAL_INDEX => Some(codes::LEXICAL_COVERAGE_GAP),
        _ => None,
    }
}

#[cfg(test)]
#[path = "repair_tests.rs"]
mod repair_tests;
