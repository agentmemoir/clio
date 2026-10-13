// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unified `clio status` report shape, its
//! JSON/text rendering, and the lenient `docker compose ps` parser.
//!
//! ## Owns
//! - The `StatusReport` aggregate and its sub-views (setup, database, compose,
//!   diagnose)
//! - The healthy/degraded verdict (`ok`) and the text renderer
//! - Parsing `docker compose ps --format json` output into service states
//!
//! ## Does not own
//! - Probing Docker, the database, or ports (see `status_cli`, `port_scan`)
//! - Credential redaction and config masking (owned by `clio-ops` / `clio-config`)
//!
//! ## Boundary
//!
//! Changes to the status field names or text layout should normally require
//! changes here. Changes to how each field is probed should not.
//!
//! Keep this module focused on the status report model and rendering.

use clio_ops::{OpsFinding, VERIFY_POINTER};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The one structured status report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct StatusReport {
    /// Overall verdict: healthy when diagnose is ok and the backend is reachable.
    pub ok: bool,
    /// Setup / deployment-config view.
    pub setup: SetupView,
    /// Resolved database backend and redacted URL.
    pub database: DatabaseView,
    /// Occupied `34300-34309` listener ports.
    pub ports: Vec<u16>,
    /// Compose service-state view.
    pub compose: ComposeView,
    /// Embedded diagnose verdict and findings.
    pub diagnose: DiagnoseView,
    /// The deeper check this report does not run: its `ok` covers setup,
    /// database, listeners, Compose, and diagnose only.
    pub verify_pointer: &'static str,
}

/// Setup / deployment-config view.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SetupView {
    /// Deployment-config path, only when the file exists; otherwise `null`.
    pub deployment_config: Option<String>,
    /// Deployment-overlay `embed.*` values, when any are set.
    pub embed: Option<ServiceConf>,
    /// Deployment-overlay `rerank.*` values, when any are set.
    pub rerank: Option<ServiceConf>,
    /// Secret-bearing deployment fields, already masked.
    pub secrets: BTreeMap<String, String>,
}

/// Non-secret provider configuration for one service.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct ServiceConf {
    /// Configured provider name (`""` when unset).
    pub provider: String,
    /// Configured endpoint URL (non-secret).
    pub url: String,
    /// Configured model id.
    pub model: String,
}

/// Resolved database backend and its credential-redacted URL.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DatabaseView {
    /// `sqlite` or `postgres`.
    pub backend: String,
    /// Resolved URL or path, with any URL userinfo redacted.
    pub url: String,
    /// Encryption and locality sentence for the resolved address, from
    /// `connection_text::database_encryption_locality`.
    pub encryption: String,
    /// Where the Clio encryption key lives, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_location: Option<String>,
    /// Why the key location could not be read, when it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_error: Option<String>,
}

/// Compose service-state view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ComposeView {
    /// No materialized `compose.yml` in the compose root.
    NotMaterialized,
    /// Docker or the compose project could not be read.
    Unavailable {
        /// Redacted, single-line reason.
        reason: String,
    },
    /// The compose project was read; service states follow.
    Available {
        /// One entry per service, sorted by service name.
        services: Vec<ServiceState>,
    },
}

/// One Compose service and its reported state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ServiceState {
    /// Compose service name.
    pub service: String,
    /// Reported container state (e.g. `running`, `exited`).
    pub state: String,
}

/// Embedded diagnose verdict.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DiagnoseView {
    /// Diagnose aggregate `ok` bit.
    pub ok: bool,
    /// Backend ping succeeded.
    pub backend_reachable: bool,
    /// Findings in deterministic order.
    pub findings: Vec<OpsFinding>,
    /// Redacted error text when the store/embedder could not be opened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Build the report and compute the overall `ok` verdict.
#[must_use]
pub(crate) fn assemble(
    setup: SetupView,
    database: DatabaseView,
    ports: Vec<u16>,
    compose: ComposeView,
    diagnose: DiagnoseView,
) -> StatusReport {
    let ok = diagnose.ok && diagnose.backend_reachable;
    StatusReport {
        ok,
        setup,
        database,
        ports,
        compose,
        diagnose,
        verify_pointer: VERIFY_POINTER,
    }
}

/// Serialize the report as one JSON line.
#[must_use]
pub(crate) fn to_json(report: &StatusReport) -> String {
    serde_json::to_string(report).unwrap_or_else(|_| "{}".to_owned())
}

/// Render the report as a short human-readable block.
#[must_use]
pub(crate) fn render_text(report: &StatusReport) -> String {
    let mut out = String::new();
    let verdict = if report.ok { "healthy" } else { "degraded" };
    let _ = writeln!(out, "clio status: {verdict}");
    let setup = &report.setup;
    let _ = writeln!(
        out,
        "  setup:    config={}",
        setup.deployment_config.as_deref().unwrap_or("none")
    );
    let _ = writeln!(
        out,
        "            embed={} rerank={}",
        service_text(setup.embed.as_ref()),
        service_text(setup.rerank.as_ref())
    );
    if !setup.secrets.is_empty() {
        let masked: Vec<String> = setup
            .secrets
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let _ = writeln!(out, "            secrets: {}", masked.join(" "));
    }
    let _ = writeln!(
        out,
        "  database: {} {} ({})",
        report.database.backend, report.database.url, report.database.encryption
    );
    if let Some(location) = &report.database.key_location {
        let _ = writeln!(out, "            encryption key: {location}");
    }
    if let Some(reason) = &report.database.key_error {
        let _ = writeln!(
            out,
            "            encryption key: could not be read ({reason})"
        );
    }
    let ports = if report.ports.is_empty() {
        "none".to_owned()
    } else {
        report
            .ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(out, "  ports:    {ports}");
    let _ = writeln!(out, "  compose:  {}", compose_text(&report.compose));
    let _ = writeln!(
        out,
        "  diagnose: {} (backend reachable: {})",
        if report.diagnose.ok { "ok" } else { "not ok" },
        report.diagnose.backend_reachable
    );
    if let Some(error) = &report.diagnose.error {
        let _ = writeln!(out, "            error: {error}");
    }
    for finding in &report.diagnose.findings {
        let _ = writeln!(
            out,
            "            [{:?}] {} {}: {}",
            finding.severity, finding.code, finding.scope, finding.message
        );
    }
    let _ = writeln!(
        out,
        "  verify:   deeper index-coverage and referential-integrity checks: `{}`",
        report.verify_pointer
    );
    out
}

fn service_text(conf: Option<&ServiceConf>) -> String {
    match conf {
        Some(conf) => format!(
            "{} {} ({})",
            if conf.provider.is_empty() {
                "default"
            } else {
                conf.provider.as_str()
            },
            conf.url,
            conf.model
        ),
        None => "unset".to_owned(),
    }
}

fn compose_text(view: &ComposeView) -> String {
    match view {
        ComposeView::NotMaterialized => "not materialized".to_owned(),
        ComposeView::Unavailable { reason } => format!("unavailable: {reason}"),
        ComposeView::Available { services } if services.is_empty() => "no services".to_owned(),
        ComposeView::Available { services } => services
            .iter()
            .map(|s| format!("{}={}", s.service, s.state))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Parse `docker compose ps --format json` output leniently: a JSON array, a
/// single JSON object, or newline-delimited JSON (older Compose versions).
/// Entries without a service name are skipped; blank input yields an empty list.
#[must_use]
pub(crate) fn parse_compose_ps(text: &str) -> Vec<ServiceState> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let values: Vec<Value> = match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Array(items)) => items,
        Ok(value @ Value::Object(_)) => vec![value],
        _ => trimmed
            .lines()
            .filter_map(|line| serde_json::from_str(line.trim()).ok())
            .collect(),
    };
    let mut services: Vec<ServiceState> = values.iter().filter_map(service_state).collect();
    services.sort_by(|a, b| a.service.cmp(&b.service));
    services
}

fn service_state(value: &Value) -> Option<ServiceState> {
    let service = value
        .get("Service")
        .or_else(|| value.get("Name"))?
        .as_str()?
        .to_owned();
    let state = value
        .get("State")
        .or_else(|| value.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    Some(ServiceState { service, state })
}

#[cfg(test)]
#[path = "status_report_tests.rs"]
mod tests;
