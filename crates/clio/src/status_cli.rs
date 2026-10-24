// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-only `clio status` command surface:
//! flag parsing, the real probe wiring (deployment config, database, ports,
//! Compose, diagnose), and the healthy/degraded/usage exit-code mapping.
//!
//! ## Owns
//! - The `clio status` dispatcher, `--output json|text`, and exit codes
//! - Assembling the injected probes into one [`StatusReport`]
//! - The store open/diagnose read path (no repair, no space reconcile)
//! - The Compose service-state probe over the existing Docker runner seam
//!
//! ## Does not own
//! - The report model and rendering (see `status_report`)
//! - The loopback port scan (see `port_scan`)
//! - Docker argument shapes (see `compose_docker`) or config masking (`clio-config`)
//!
//! ## Boundary
//!
//! Changes to how `status` probes or maps exit codes should normally require
//! changes here. Changes to the report layout should not.
//!
//! Keep this module focused on the `clio status` command surface.

use crate::cli_error::CliError;
use crate::cli_top_level::report_surface_usage_error;
use crate::compose_docker::{ComposeTarget, DockerRunner, RealDocker, Stack, compose_args};
use crate::compose_env;
use crate::connection_text;
use crate::port_scan;
use crate::status_report::{
    ComposeView, DatabaseView, DiagnoseView, ServiceConf, SetupView, StatusReport, assemble,
    parse_compose_ps, render_text, to_json,
};
use clio_config::db_path::ResolvedDatabase;
use clio_index::Embedder;
use clio_ops::{DiagnoseArgs, OpsFinding, Severity, codes, redact_credentials, scope_label};
use clio_store::{DekProvider, IndexStore, LocalDevKms, OpsStore, PostgresStore, SqliteStore};
use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

/// The two backend reads `diagnose` needs from one concrete store.
pub(crate) trait StatusRepos: OpsStore + IndexStore {}
impl<T: OpsStore + IndexStore> StatusRepos for T {}

/// Everything `clio status` reads, injectable so the command core never
/// depends on the host environment in tests.
pub(crate) trait StatusDeps {
    /// Environment lookup for `--db` / backend precedence.
    fn env(&self, key: &str) -> Option<String>;
    /// Open the selected backend read path (never reconciles the space).
    fn open_store(&self, resolved: &ResolvedDatabase) -> Result<Box<dyn StatusRepos>, AmError>;
    /// Build the optional embedding sidecar probe.
    fn embedder(&self) -> Result<Option<Arc<dyn Embedder>>, AmError>;
    /// Occupied `34300-34309` listener ports.
    fn scan_ports(&self) -> Vec<u16>;
    /// Compose service state for a compose root.
    fn compose(&self, root: &Path) -> ComposeView;
    /// Setup / deployment-config view.
    fn setup(&self) -> SetupView;
    /// Where the Clio encryption key lives: the raw location string when a key
    /// exists, `Ok(None)` when none, `Err(reason)` when the store could not be
    /// consulted.
    fn key_location(&self) -> Result<Option<String>, String>;
    /// Caller timestamp (determinism).
    fn timestamp(&self) -> String;
    /// Compose root default when `--dir` is absent.
    fn cwd(&self) -> Option<PathBuf>;
}

/// Requested rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Output {
    /// Human-readable block (default).
    Text,
    /// One JSON line.
    Json,
}

/// `clio status` entrypoint: wire the real probes and process environment.
pub(crate) fn status(args: impl IntoIterator<Item = String>) -> ExitCode {
    let raw: Vec<String> = args.into_iter().collect();
    status_with(&raw, &RealStatus)
}

/// Command core over injected probes: render the report and map its verdict.
fn status_with(raw: &[String], deps: &dyn StatusDeps) -> ExitCode {
    if raw
        .iter()
        .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
    {
        print_help();
        return ExitCode::SUCCESS;
    }
    match build(raw, deps) {
        Ok((report, output)) => {
            match output {
                Output::Text => print!("{}", render_text(&report)),
                Output::Json => println!("{}", to_json(&report)),
            }
            if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(error) => {
            report_surface_usage_error(&error, raw);
            error.exit_code()
        }
    }
}

/// Build the report from the injected probes. The error case is a usage error.
fn build(raw: &[String], deps: &dyn StatusDeps) -> Result<(StatusReport, Output), CliError> {
    let names = crate::cli_command_registry_surface::parse_flag_names("status");
    let flags = crate::cli_top_level::parse_flags(raw.iter().cloned(), &names)?;
    let output = match flags.get("output").map(String::as_str) {
        None | Some("text") => Output::Text,
        Some("json") => Output::Json,
        Some(other) => {
            return Err(CliError::usage(format!(
                "unknown --output `{other}`; expected json|text"
            )));
        }
    };
    let resolved = match crate::db_target::resolve_target(
        &|key| deps.env(key),
        flags.get("db").map(String::as_str),
        flags.get("backend").map(String::as_str),
    ) {
        Ok(resolved) => resolved,
        Err(reason) => return Err(CliError::usage(reason)),
    };
    let (key_location, key_error) = match deps.key_location() {
        Ok(location) => (location, None),
        Err(reason) => (None, Some(redact_credentials(&reason))),
    };
    let database = DatabaseView {
        backend: resolved.backend.clone(),
        url: redact_credentials(&resolved.url),
        encryption: connection_text::database_encryption_locality(&resolved.url),
        key_location,
        key_error,
    };
    let ports = deps.scan_ports();
    let root = flags
        .get("dir")
        .map(PathBuf::from)
        .or_else(|| deps.cwd())
        .unwrap_or_else(|| PathBuf::from("."));
    let compose = deps.compose(&root);
    let diagnose = match deps.open_store(&resolved) {
        Ok(store) => match deps.embedder() {
            Ok(embedder) => diagnose_view(&*store, embedder.as_deref(), &deps.timestamp()),
            Err(e) => failed_diagnose(codes::EMBEDDER_UNAVAILABLE, &e.message),
        },
        Err(e) => failed_diagnose(codes::BACKEND_UNREACHABLE, &e.message),
    };
    Ok((
        assemble(deps.setup(), database, ports, compose, diagnose),
        output,
    ))
}

/// Run diagnose read-only against an open store (no space reconcile).
fn diagnose_view(
    store: &dyn StatusRepos,
    embedder: Option<&dyn Embedder>,
    timestamp: &str,
) -> DiagnoseView {
    let report = clio_ops::diagnose_whole_store(&DiagnoseArgs {
        store,
        index: store,
        embedder,
        timestamp,
    });
    DiagnoseView {
        ok: report.ok,
        backend_reachable: report.backend_reachable,
        findings: redact_findings(report.findings),
        error: None,
    }
}

/// Driver error text can embed connection URLs; funnel every finding message
/// through the shared redactor (status must never echo secrets).
fn redact_findings(findings: Vec<OpsFinding>) -> Vec<OpsFinding> {
    findings
        .into_iter()
        .map(|mut finding| {
            finding.message = redact_credentials(&finding.message);
            finding
        })
        .collect()
}

/// A degraded diagnose view for a store/embedder that could not be read.
fn failed_diagnose(code: &str, message: &str) -> DiagnoseView {
    DiagnoseView {
        ok: false,
        backend_reachable: false,
        findings: vec![OpsFinding::new(
            code,
            Severity::Error,
            scope_label(None),
            redact_credentials(message),
        )],
        error: Some(redact_credentials(message)),
    }
}

/// Open the selected backend through the shared precedence resolver, without
/// reconciling the embedding space (status never mutates the store).
fn open_chosen(resolved: &ResolvedDatabase) -> Result<Box<dyn StatusRepos>, AmError> {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    match resolved.backend.as_str() {
        "sqlite" => {
            let store = if resolved.url == "sqlite::memory:" {
                SqliteStore::open_in_memory(kms)?
            } else {
                // `status` is read-only: a missing file must degrade, never let
                // `SqliteStore::open` create the database and run migrations.
                let path = Path::new(&resolved.url);
                if !path.exists() {
                    return Err(AmError::new(
                        ErrorCode::NotFound,
                        format!("database file does not exist: {}", resolved.url),
                    ));
                }
                SqliteStore::open(path, kms)?
            };
            Ok(Box::new(store))
        }
        "postgres" => Ok(Box::new(PostgresStore::open(&resolved.url, kms)?)),
        other => Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("backend must be sqlite|postgres, got `{other}`"),
        )),
    }
}

/// The real probe wiring.
struct RealStatus;

impl StatusDeps for RealStatus {
    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn open_store(&self, resolved: &ResolvedDatabase) -> Result<Box<dyn StatusRepos>, AmError> {
        open_chosen(resolved)
    }

    fn embedder(&self) -> Result<Option<Arc<dyn Embedder>>, AmError> {
        clio_mcp::runtime::build_ops_embedder()
    }

    fn scan_ports(&self) -> Vec<u16> {
        port_scan::listening_ports()
    }

    fn compose(&self, root: &Path) -> ComposeView {
        probe_compose(
            &RealDocker,
            root,
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
    }

    fn setup(&self) -> SetupView {
        let path = clio_config::resolve_deployment_path(None);
        let rt = clio_config::Runtime::new();
        build_setup(&rt, path.as_deref())
    }

    fn key_location(&self) -> Result<Option<String>, String> {
        clio_config::secret_key::location()
            .map(|found| found.map(|location| location.to_string()))
            .map_err(|e| e.message)
    }

    fn timestamp(&self) -> String {
        clio_mcp::runtime::now_utc()
    }

    fn cwd(&self) -> Option<PathBuf> {
        std::env::current_dir().ok()
    }
}

/// Read the Compose project's service state through the existing runner seam.
/// A missing project, unsupported platform, absent Docker, or non-zero Compose
/// exit all degrade to a reported [`ComposeView`] rather than failing status.
fn probe_compose(runner: &dyn DockerRunner, root: &Path, os: &str, arch: &str) -> ComposeView {
    if !root.join("compose.yml").exists() {
        return ComposeView::NotMaterialized;
    }
    let target = match ComposeTarget::detect(os, arch) {
        Ok(target) => target,
        Err(reason) => return ComposeView::Unavailable { reason },
    };
    let stack = compose_env::read_marker(root)
        .and_then(|raw| Stack::parse_selection(&raw).ok())
        .unwrap_or(Stack::PostgresOnly);
    let args = compose_args(target, stack, "ps", &["-a", "--format", "json"]);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match runner.capture(&arg_refs, root) {
        Ok((0, text)) => ComposeView::Available {
            services: parse_compose_ps(&text),
        },
        Ok((_code, text)) => ComposeView::Unavailable {
            reason: one_line(&text),
        },
        Err(e) => ComposeView::Unavailable {
            reason: redact_credentials(&format!("docker CLI not found: {e}")),
        },
    }
}

/// Collapse output to one redacted line for the reason field.
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = redact_credentials(&flat);
    if redacted.is_empty() {
        "docker compose ps failed".to_owned()
    } else {
        redacted.chars().take(200).collect()
    }
}

/// Build the setup view from an already-booted runtime and its deployment path.
/// Every secret-bearing overlay key is masked with the shared masker (discovered
/// from the overlay itself, not a fixed allowlist).
fn build_setup(rt: &clio_config::Runtime, path: Option<&Path>) -> SetupView {
    let present = path.filter(|p| p.exists());
    let mut secrets = BTreeMap::new();
    for field in rt.deployment_keys() {
        if !clio_config::secret::path_is_secret(field) {
            continue;
        }
        if let Some(value) = rt.deployment_field(field) {
            secrets.insert(
                field.to_owned(),
                clio_config::secret::mask_secret(&scalar(value)),
            );
        }
    }
    SetupView {
        deployment_config: present.map(|p| p.display().to_string()),
        embed: service_conf(rt, "embed"),
        rerank: service_conf(rt, "rerank"),
        secrets,
    }
}

/// Read one service's deployment-overlay fields, when any are set.
fn service_conf(rt: &clio_config::Runtime, name: &str) -> Option<ServiceConf> {
    let read = |field: &str| {
        rt.deployment_field(&format!("{name}.{field}"))
            .map(scalar)
            .unwrap_or_default()
    };
    let conf = ServiceConf {
        provider: read("provider"),
        url: read("url"),
        model: read("model"),
    };
    (conf != ServiceConf::default()).then_some(conf)
}

/// Scalar text for a JSON overlay value (strings unquoted; others compact).
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn print_help() {
    println!(
        "{}\n Read-only stack health: setup/deployment config, resolved database\n \
         (redacted URL), 34300-34309 listeners, Compose service state, and\n \
         the `ops diagnose` verdict. Never writes. Deeper index-coverage and\n \
         referential-integrity checks are outside this scope: run `clio ops verify`.\n \
         Exit codes: 0 healthy, 1 degraded, 2 usage.",
        crate::cli_help_usage::usage_line("status").unwrap_or_else(|| "clio status".to_owned())
    );
}

#[cfg(test)]
#[path = "status_cli_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "status_cli_probe_tests.rs"]
mod status_cli_probe_tests;

#[cfg(test)]
#[path = "status_cli_fault_tests.rs"]
mod fault_tests;

/// The agreement between `status` and the applied summary, in their own module.
///
/// Split out rather than added to `status_cli_tests.rs`, which is at the
/// workspace's line limit.
#[cfg(test)]
#[path = "status_cli_agreement_tests.rs"]
mod agreement_tests;
