// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio ops` CLI surface: dispatching the
//! operator health commands and mapping their outcomes to process exit codes.
//!
//! ## Owns
//! - The `clio ops` subcommand dispatcher and its flag handling, including the
//!   documented global `--output` mode selection
//! - Opening the operator store/index/embedder and emitting report results
//! - The documented exit-code contract for the operator commands
//!
//! ## Does not own
//! - The operator checks and repair policy (`clio-ops`)
//! - The report text renderings and output emitters (`ops_cli_render`)
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
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use crate::cli_output::OutputMode;
use crate::ops_cli_actions::{run_doctor, run_reindex, run_repair};
use crate::ops_cli_render::{
    emit_error, emit_report, emit_usage_error, render_diagnose, render_verify, report_output_error,
};

/// Exit-code contract for the operator commands.
///
/// - `0`: success — `diagnose` and `verify` are healthy, `doctor` composed a
///   clean plan, `repair` applied, or `reindex` applied its sweep.
/// - `1`: operator findings, store/index or key-store failures, missing repair
///   plans, unreachable diagnose backends, or failed repair actions.
/// - `2`: a repair was refused (missing confirm or unknown action), the
///   backend value is invalid, or the `clio ops` subcommand is unknown.
/// - `3`: `reindex` refused, failed, or ran report-only (`--dry-run`): a report
///   applies nothing, so it is never `ok`.
/// - `75`: a Postgres lock conflict is temporary; retry the command.
///
/// `doctor` never mutates the store under any `--dry-run` value. `repair` and
/// `reindex` mutate only with `--confirm`, and `reindex` reads that flag by
/// value: a bare `--confirm` starts the live rebuild, `--confirm false` does not,
/// and pairing it with `--dry-run` is refused.
///
/// Plan ids are process-local: `doctor` and `repair` run as separate processes,
/// so pass an explicit `--actions` list for a cross-invocation repair.
pub(crate) fn ops(args: impl IntoIterator<Item = String>) -> ExitCode {
    ops_with(args, &|key| std::env::var(key).ok())
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
    let mut stdout = std::io::stdout();
    ops_with_out(args, get, &mut stdout)
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

/// The same surface against a caller-supplied output writer, so a test can read
/// what `diagnose` and `verify` printed instead of racing the real stdout.
pub(crate) fn ops_with_out<F>(
    args: impl IntoIterator<Item = String>,
    get: &F,
    out: &mut dyn Write,
) -> ExitCode
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
    // The documented global `--output` selects the renderer, the way every other
    // command surface resolves it; an invalid value reports like the other
    // surfaces too.
    let is_tty = crate::cli_output::stdout_is_tty();
    let mode =
        match crate::cli_output::resolve_output(flags.get("output").map(String::as_str), is_tty) {
            Ok(mode) => mode,
            Err(error) => return report_output_error(&error, &raw, is_tty),
        };
    match sub {
        "diagnose" | "verify" | "doctor" | "repair" | "reindex" => {
            run_backend(sub, &flags, get, mode, out)
        }
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
    let lines = crate::cli_help_surfaces::ops_usage_lines(sub)
        .expect("only a real ops subcommand reaches this help branch");
    lines.join("\n")
}

/// Every backend view the operator commands need from one concrete store.
pub(crate) trait OpsRepos: Store + OpsStore + IndexStore {}
impl<T: Store + OpsStore + IndexStore> OpsRepos for T {}

/// Open the selected backend through the shared precedence resolver and run
/// the command against its concrete store.
fn run_backend<F>(
    sub: &str,
    flags: &HashMap<String, String>,
    get: &F,
    mode: OutputMode,
    out: &mut dyn Write,
) -> ExitCode
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
    let kms: Arc<dyn DekProvider> = if matches!(sub, "reindex" | "repair") {
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
                    Ok(()) => dispatch(sub, flags, &store, get, mode, out),
                    Err(e) => emit_error(&e, 1),
                },
                Err(e) => emit_error(&e, 1),
            }
        }
        "postgres" => match PostgresStore::open(database, kms) {
            Ok(mut store) => match reconcile_space(&mut store, get) {
                Ok(()) => dispatch(sub, flags, &store, get, mode, out),
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
///
/// `doctor`, `repair`, and `reindex` keep their JSON-only emission; only the
/// read-only `diagnose` and `verify` reports follow the resolved output mode.
fn dispatch<S, F>(
    sub: &str,
    flags: &HashMap<String, String>,
    store: &S,
    get: &F,
    mode: OutputMode,
    out: &mut dyn Write,
) -> ExitCode
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
        "diagnose" => run_diagnose(store, embedder.as_deref(), &timestamp, mode, out),
        "verify" => run_verify(store, bank.as_deref(), &timestamp, mode, out),
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
    mode: OutputMode,
    out: &mut dyn Write,
) -> ExitCode {
    let report = clio_ops::diagnose_unscoped(&clio_ops::DiagnoseArgs {
        store,
        index: store,
        embedder,
        timestamp,
    });
    emit_report(&report, &render_diagnose(&report), mode, out);
    if report.ok && report.backend_reachable {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn run_verify<S: OpsRepos>(
    store: &S,
    bank: Option<&str>,
    timestamp: &str,
    mode: OutputMode,
    out: &mut dyn Write,
) -> ExitCode {
    match clio_ops::verify(&clio_ops::VerifyArgs {
        store,
        index: store,
        bank,
        timestamp,
    }) {
        Ok(report) => {
            emit_report(&report, &render_verify(&report), mode, out);
            if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => emit_error(&e, 1),
    }
}

#[cfg(test)]
#[path = "ops_cli_fault_tests.rs"]
mod ops_cli_fault_tests;

#[cfg(test)]
#[path = "ops_cli_open_tests.rs"]
mod ops_cli_open_tests;

#[cfg(test)]
#[path = "ops_cli_output_tests.rs"]
mod ops_cli_output_tests;

#[cfg(test)]
#[path = "ops_cli_tests.rs"]
mod tests;
