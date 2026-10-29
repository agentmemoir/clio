// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the three `clio ops` subcommand handlers that
//! plan, mutate, or rebuild: `doctor`, `repair`, and `reindex`, including their
//! outcome-to-exit-code mapping.
//!
//! ## Owns
//! - `run_doctor`: repair-plan composition and its exit codes
//! - `run_repair`: confirmation-gated, allowlisted repair and its exit codes
//! - `run_reindex`: the dry-run/live rebuild UX and its exit codes
//!
//! ## Does not own
//! - The `clio ops` dispatch, flag handling, and output-mode selection
//!   (`ops_cli`)
//! - The read-only `diagnose`/`verify` handlers and their mode-resolved
//!   emission (`ops_cli`, `ops_cli_render`)
//! - The check, plan, and rebuild semantics (`clio-ops`, `clio_index`)
//!
//! ## Boundary
//!
//! Changes to what `doctor`, `repair`, or `reindex` apply, or to which exit
//! code their outcomes map to, should normally require changes here. Changes
//! to the `clio ops` command surface should not require changes here.
//!
//! These three handlers keep their one-line JSON emission; only the read-only
//! verify and diagnose reports follow the resolved output mode.

use crate::ops_cli::OpsRepos;
use crate::ops_cli_render::{emit_error, emit_usage_error, print_json};
use clio_index::Embedder;
use std::collections::HashMap;
use std::process::ExitCode;

/// The `doctor` handler: compose (or record) the repair plan.
pub(crate) fn run_doctor<S: OpsRepos>(
    store: &S,
    bank: Option<&str>,
    flags: &HashMap<String, String>,
    timestamp: &str,
    plans: &clio_ops::PlanRegistry,
) -> ExitCode {
    let dry_run = flags.get("dry-run").is_none_or(|v| v != "false");
    match clio_ops::doctor_unscoped(&clio_ops::DoctorArgs {
        store,
        index: store,
        bank,
        dry_run,
        timestamp,
        plans,
    }) {
        Ok(plan) => {
            print_json(&plan);
            if plan.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => emit_error(&e, 1),
    }
}

/// The `repair` handler: apply the allowlisted actions under `--confirm`.
pub(crate) fn run_repair<S: OpsRepos>(
    store: &S,
    bank: Option<&str>,
    flags: &HashMap<String, String>,
    timestamp: &str,
    plans: &clio_ops::PlanRegistry,
    embedder: Option<&dyn Embedder>,
) -> ExitCode {
    let actions: Vec<String> = flags
        .get("actions")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    match clio_ops::repair(&clio_ops::RepairArgs {
        store,
        writer: store,
        index: store,
        embedder,
        plans,
        plan_id: flags.get("plan").map(String::as_str),
        actions: &actions,
        bank,
        confirm: flags.contains_key("confirm"),
        timestamp,
    }) {
        Ok(outcome) => {
            print_json(&outcome);
            if outcome.ok {
                ExitCode::SUCCESS
            } else if outcome
                .findings
                .iter()
                .any(|finding| finding.code == clio_ops::codes::PLAN_NOT_FOUND)
            {
                ExitCode::from(1)
            } else if !outcome.findings.is_empty() {
                ExitCode::from(2)
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => emit_error(&e, 1),
    }
}

/// The `reindex` handler: rebuild the derived index, report-only without
/// `--confirm`.
pub(crate) fn run_reindex<S: OpsRepos>(
    store: &S,
    bank: Option<&str>,
    flags: &HashMap<String, String>,
    embedder: Option<&dyn Embedder>,
) -> ExitCode {
    let target = match flags.get("target") {
        Some(raw) => match clio_index::RebuildTargets::parse(raw) {
            Ok(target) => target,
            Err(e) => return emit_usage_error(e.message),
        },
        None => clio_index::RebuildTargets::All,
    };
    // `--confirm` alone starts the live rebuild, the way `repair` treats it. The
    // value decides, not presence: the parser stores the token after a bare flag,
    // so `--confirm false` is not a confirmation. `--dry-run true` asks for a report.
    let confirm = flags.get("confirm").is_some_and(|v| v != "false");
    let dry_run = match flags.get("dry-run") {
        Some(v) => v != "false",
        None => !confirm,
    };
    match clio_ops::reindex(&clio_ops::ReindexArgs {
        store,
        index: store,
        embedder,
        bank,
        target,
        dry_run,
        confirm,
    }) {
        Ok(report) => {
            print_json(&report);
            if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(3)
            }
        }
        Err(e) => emit_error(&e, 3),
    }
}
