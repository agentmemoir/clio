// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio ops` CLI surface: dispatching the
//! operator health commands and mapping their outcomes to process exit codes.
//!
//! ## Owns
//! - The `clio ops` subcommand dispatcher and its flag handling
//! - Opening the operator store/index/embedder and rendering JSON results
//! - The documented exit-code contract for the operator commands
//!
//! ## Does not own
//! - The operator checks and repair policy (`clio-ops`)
//! - Backend SQL (`clio-store`) or the MCP transports (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how operator commands map to exit codes should normally require
//! changes here. Changes to operator check semantics should not require
//! changes here.
//!
//! Keep this module focused on the `clio ops` command surface and exit codes.

use crate::cli_top_level::report_surface_usage_error;
use clio_index::Embedder;
use clio_store::{
    DekProvider, IndexStore, LocalDevKms, OpsStore, PostgresStore, SqliteStore, Store,
};
use clio_types::AmError;
use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

/// Exit-code contract for the operator commands.
///
/// - `0`: success — `diagnose` and `verify` are healthy, `doctor` composed a
///   clean plan, `repair` applied, or `reindex` completed.
/// - `1`: `verify` or `doctor` reported error-severity findings, a store/index
///   read failed, a referenced repair plan was not found, the `diagnose`
///   backend was unreachable, a repair action failed, or the store could not
///   be opened.
/// - `2`: a repair was refused (missing confirm or unknown action), the
///   backend value is invalid, or the `clio ops` subcommand is unknown.
/// - `3`: `reindex` failed or aborted (including a dense target with no
///   embedding sidecar).
///
/// `doctor` never mutates the store under any `--dry-run` value; `repair` and
/// `reindex` mutate only when `--confirm` is given.
///
/// Plan ids are process-local: `doctor` and `repair` run as separate CLI
/// processes, so a `--plan` id from an earlier invocation is not resolvable.
/// Cross-invocation repairs pass an explicit `--actions` list instead.
pub(crate) fn ops(args: impl IntoIterator<Item = String>) -> ExitCode {
    ops_with(args, &|key| std::env::var(key).ok())
}

/// The flag names each `clio ops` subcommand parses with, from its record. A
/// subcommand with no record keeps the old empty list: only the globals and
/// the help spelling are accepted, and the dispatcher refuses the subcommand
/// itself. A flag a sibling subcommand owns is a usage error rather than a
/// silently ignored token.
fn ops_flag_names(sub: &str) -> Vec<&'static str> {
    match sub {
        "diagnose" | "verify" | "doctor" | "repair" | "reindex" => {
            crate::cli_command_registry_surface::parse_flag_names(&format!("ops {sub}"))
        }
        _ => Vec::new(),
    }
}

/// `ops` against an injected environment lookup.
///
/// Every other command surface reads its environment through a lookup the
/// caller supplies (`cli_read`, `sync_cli`, `backup_cli`, `mcp_cli`,
/// `status_cli`); this one read `std::env::var` inline in three places, so a
/// test here had no way to express "this machine has no deployment config" and
/// its verdict depended on the developer's own sidecar setup (#90). The seam is
/// the same one `mcp_cli`'s `mcp_options_with` already provides.
pub(crate) fn ops_with<F>(args: impl IntoIterator<Item = String>, get: &F) -> ExitCode
where
    F: Fn(&str) -> Option<String>,
{
    let raw: Vec<String> = args.into_iter().collect();
    let (sub, rest) = raw
        .split_first()
        .map_or(("", &[][..]), |(s, r)| (s.as_str(), r));
    let flag_names = ops_flag_names(sub);
    let flags = match crate::cli_top_level::parse_flags(rest.to_vec(), &flag_names) {
        Ok(flags) => flags,
        Err(error) => return report_surface_usage_error(&error, &raw),
    };
    if matches!(sub, "help" | "--help" | "-h") || flags.contains_key("help") {
        // The subcommand asked for decides which line is printed, so `ops repair
        // --help` names the flags repair takes rather than the flags diagnose
        // takes. A bare `ops --help` lists all five.
        let asked = match sub {
            "diagnose" | "verify" | "doctor" | "repair" | "reindex" => sub.to_owned(),
            _ => String::new(),
        };
        println!("{}\n", ops_usage(&asked));
        println!("{}", crate::cli_top_level::database_help_block(get));
        return ExitCode::SUCCESS;
    }
    match sub {
        "diagnose" | "verify" | "doctor" | "repair" | "reindex" => run_backend(sub, &flags, get),
        other => {
            eprintln!(
                "unknown ops command `{other}`; try `clio ops diagnose|verify|doctor|repair|reindex`"
            );
            ExitCode::from(2)
        }
    }
}

/// The `ops` usage lines: the requested subcommand's own, or all five joined.
fn ops_usage(sub: &str) -> String {
    let lines: Vec<&str> = crate::cli_help_surfaces::ops_usage_lines(sub)
        .expect("only a real ops subcommand reaches this help branch");
    lines.join("\n")
}

/// Every backend view the operator commands need from one concrete store.
trait OpsRepos: Store + OpsStore + IndexStore {}
impl<T: Store + OpsStore + IndexStore> OpsRepos for T {}

/// Subcommands that decrypt item content and therefore need the real key store.
fn needs_store_keys(sub: &str) -> bool {
    matches!(sub, "reindex" | "repair")
}

/// Open the selected backend through the shared precedence resolver and run
/// the command against its concrete store.
fn run_backend<F>(sub: &str, flags: &HashMap<String, String>, get: &F) -> ExitCode
where
    F: Fn(&str) -> Option<String>,
{
    let resolved = match crate::db_target::resolve_target(
        get,
        flags.get("db").map(String::as_str),
        flags.get("backend").map(String::as_str),
    ) {
        Ok(resolved) => resolved,
        Err(reason) => {
            return emit_error(
                &AmError::new(clio_types::ErrorCode::ConfigCorrupt, reason),
                1,
            );
        }
    };
    let backend = resolved.backend.as_str();
    let database = resolved.url.as_str();
    // Only the subcommands that re-derive state read sealed item content, and
    // only they need the store's key material. They must resolve the same key
    // store the read and write surfaces use: an empty process-local KMS made
    // every item unreadable, so a rebuild reported success while rewriting
    // nothing. Read-only health commands keep a process-local KMS, so they
    // still answer on a store whose key file is absent or damaged.
    let kms: Arc<dyn DekProvider> = if needs_store_keys(sub) {
        match clio_config::kms_path::resolve_kms_path(get, database) {
            Some(path) => match LocalDevKms::open_persistent(path) {
                Ok(kms) => Arc::new(kms),
                Err(e) => return emit_error(&e, 1),
            },
            None => Arc::new(LocalDevKms::new()),
        }
    } else {
        Arc::new(LocalDevKms::new())
    };
    match backend {
        "sqlite" => {
            let opened = if database == "sqlite::memory:" {
                SqliteStore::open_in_memory(kms)
            } else {
                SqliteStore::open(Path::new(database), kms)
            };
            match opened {
                Ok(mut store) => match reconcile_space(&mut store, get) {
                    Ok(()) => dispatch(sub, flags, &store, get),
                    Err(e) => emit_error(&e, 1),
                },
                Err(e) => emit_error(&e, 1),
            }
        }
        "postgres" => match PostgresStore::open(database, kms) {
            Ok(mut store) => match reconcile_space(&mut store, get) {
                Ok(()) => dispatch(sub, flags, &store, get),
                Err(e) => emit_error(&e, 1),
            },
            Err(e) => emit_error(&e, 1),
        },
        other => emit_usage_error(format!("backend must be sqlite|postgres, got `{other}`")),
    }
}

/// Reconcile the opened store with the configured embedding space.
///
/// A mismatch without the `embed.switch_space` opt-in fails closed (exit 1)
/// rather than purging the derived dense index.
fn reconcile_space<F>(store: &mut impl clio_store::SpaceReconcile, get: &F) -> Result<(), AmError>
where
    F: Fn(&str) -> Option<String>,
{
    // `with_env`, not `new`: `new` reads the process environment, so the
    // embedding space came from the host's config file and a test could not
    // state which space it meant (#90).
    let effective = clio_config::Runtime::with_env(get).effective_raw();
    let space = clio_store::configured_embedding_space(&effective)?;
    store.reconcile_space(
        space.as_ref(),
        clio_store::space_switch_confirmed(&effective),
    )?;
    Ok(())
}

/// Dispatch one operator subcommand against an already-open store.
fn dispatch<S, F>(sub: &str, flags: &HashMap<String, String>, store: &S, get: &F) -> ExitCode
where
    S: OpsRepos,
    F: Fn(&str) -> Option<String>,
{
    let bank = flags.get("bank").cloned();
    let timestamp = clio_mcp::runtime::now_utc();
    let embedder = match clio_mcp::runtime::build_ops_embedder_with(get) {
        Ok(embedder) => embedder,
        // An unknown/misconfigured provider fails closed with the allowed
        // values instead of quietly disabling dense work.
        Err(e) => return emit_error(&e, 1),
    };
    let plans = clio_ops::PlanRegistry::new();
    match sub {
        "diagnose" => run_diagnose(store, embedder.as_deref(), &timestamp),
        "verify" => run_verify(store, bank.as_deref(), &timestamp),
        "doctor" => run_doctor(store, bank.as_deref(), flags, &timestamp, &plans),
        "repair" => run_repair(
            store,
            bank.as_deref(),
            flags,
            &timestamp,
            &plans,
            embedder.as_deref(),
        ),
        "reindex" => run_reindex(store, bank.as_deref(), flags, embedder.as_deref()),
        _ => ExitCode::from(2),
    }
}

fn run_diagnose<S: OpsRepos>(
    store: &S,
    embedder: Option<&dyn Embedder>,
    timestamp: &str,
) -> ExitCode {
    let report = clio_ops::diagnose(&clio_ops::DiagnoseArgs {
        store,
        index: store,
        embedder,
        timestamp,
    });
    print_json(&report);
    if report.ok && report.backend_reachable {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_verify<S: OpsRepos>(store: &S, bank: Option<&str>, timestamp: &str) -> ExitCode {
    match clio_ops::verify(&clio_ops::VerifyArgs {
        store,
        index: store,
        bank,
        timestamp,
    }) {
        Ok(report) => {
            print_json(&report);
            if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => emit_error(&e, 1),
    }
}

fn run_doctor<S: OpsRepos>(
    store: &S,
    bank: Option<&str>,
    flags: &HashMap<String, String>,
    timestamp: &str,
    plans: &clio_ops::PlanRegistry,
) -> ExitCode {
    let dry_run = flags.get("dry-run").is_none_or(|v| v != "false");
    match clio_ops::doctor(&clio_ops::DoctorArgs {
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

fn run_repair<S: OpsRepos>(
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

fn run_reindex<S: OpsRepos>(
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
    let dry_run = flags.get("dry-run").is_none_or(|v| v != "false");
    match clio_ops::reindex(&clio_ops::ReindexArgs {
        store,
        index: store,
        embedder,
        bank,
        target,
        dry_run,
        confirm: flags.contains_key("confirm"),
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

/// Render an error as structured JSON and return the mapped exit code.
fn emit_error(error: &AmError, code: u8) -> ExitCode {
    print_json(&serde_json::json!({
        "ok": false,
        "code": error.code.as_str(),
        "message": redact(&error.message),
    }));
    ExitCode::from(code)
}

/// Render a bad flag value with the shared usage code and exit group.
fn emit_usage_error(message: impl AsRef<str>) -> ExitCode {
    print_json(&serde_json::json!({
        "ok": false,
        "code": "usage",
        "message": redact(message.as_ref()),
    }));
    ExitCode::from(2)
}

/// Print a serializable value as one JSON line.
fn print_json(value: &impl serde::Serialize) {
    println!(
        "{}",
        serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
    );
}

/// Strip credentials from any URL embedded in a driver error message (shared
/// redactor, also applied by the MCP ops surfaces).
fn redact(message: &str) -> String {
    clio_ops::redact_credentials(message)
}

#[cfg(test)]
#[path = "ops_cli_fault_tests.rs"]
mod ops_cli_fault_tests;

#[cfg(test)]
#[path = "ops_cli_open_tests.rs"]
mod ops_cli_open_tests;

#[cfg(test)]
#[path = "ops_cli_tests.rs"]
mod tests;
