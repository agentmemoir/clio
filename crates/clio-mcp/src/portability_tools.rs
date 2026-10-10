// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP argument decoding and result shaping for
//! the JSON portability tools (`export`, `import`) and the `import_provider`
//! first-release stub.
//!
//! ## Owns
//! - `export` / `import` argument decoding into `clio-compliance` calls
//! - `import_provider` routing (Mem0 adapter or non-writing stub)
//!
//! ## Does not own
//! - Bundle semantics, gating, or manifest math (`clio-compliance`)
//! - Admission scoring (clio-admission owns the policy; this module only binds)
//!
//! ## Boundary
//!
//! Changes to tool argument shapes or result payloads should normally require
//! changes here. Changes to bundle or import behavior should not require
//! changes here.
//!
//! Keep this module focused on MCP portability dispatch.

use crate::runtime::{McpState, StderrSinks, StoreSignals};
use crate::write_tools::{gated_write_policy, opt_bool, opt_str, req_str};
use clio_compliance::{ExportContentMode, ExportFilter, ImportArgs, import as import_bundle};
use clio_types::AmError;
use serde_json::{Value, json};
use std::sync::Arc;

/// Route a portability tool name to its handler (raw result).
#[inline(never)]
pub(crate) fn dispatch_inner(state: &McpState, name: &str, args: &Value) -> Result<Value, AmError> {
    match name {
        "export" => export_tool(state, args),
        "import" => import_tool(state, args),
        "import_provider" => import_provider_tool(state, args),
        other => Err(AmError::not_implemented(other)),
    }
}

/// `export(destination, filter?, content_mode?)`: one bundle file + manifest.
#[inline(never)]
fn export_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let filter = ExportFilter {
        category: opt_str(args, "category").map(str::to_owned),
        time_start: opt_str(args, "time_start").map(str::to_owned),
        time_end: opt_str(args, "time_end").map(str::to_owned),
        include_archived: opt_bool(args, "include_archived", true),
        include_discarded: opt_bool(args, "include_discarded", true),
    };
    let content_mode = ExportContentMode::parse(opt_str(args, "content_mode"))?;
    let outcome = clio_compliance::export(&clio_compliance::ExportArgs {
        store: &*state.store,
        compliance: &*state.compliance,
        persona: &*state.persona,
        history: &*state.history,
        belief: &*state.belief,
        ctx: &ctx,
        destination: req_str(args, "destination")?,
        filter,
        content_mode,
        timestamp: &ts,
    })?;
    let manifest =
        serde_json::to_value(&outcome.manifest).expect("completeness manifest serializes");
    Ok(json!({
        "ok": true,
        "destination": outcome.destination,
        "content_mode": outcome.content_mode.as_str(),
        "manifest": manifest
    }))
}

/// `import(source, dry_run?, force?, reason?)`: manifest-validated, idempotent,
/// admission-gated restore.
#[inline(never)]
fn import_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args);
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));
    let i_args = ImportArgs {
        store: &*state.store,
        batch: &*state.batch,
        persona: &*state.persona,
        history: &*state.history,
        belief: &*state.belief,
        ctx: &ctx,
        source: req_str(args, "source")?,
        dry_run: opt_bool(args, "dry_run", false),
        force: opt_bool(args, "force", false),
        reason: opt_str(args, "reason"),
        policy: gated_write_policy(state, ctx.bank.as_str(), args),
        signals: &signals,
        sink: &StderrSinks,
        belief_signals: &signals,
        timestamp: &ts,
    };
    let report = import_bundle(&i_args)?;
    Ok(json!({
        "ok": true,
        "report": serde_json::to_value(&report).expect("import report serializes")
    }))
}

/// `import_provider(provider, credentials?, options?, dry_run?)`: routes
/// Mem0 to the real provider adapter; unimplemented providers return the
/// non-writing stub with `PROVIDER_IMPORT_UNSUPPORTED`.
#[inline(never)]
fn import_provider_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let provider = req_str(args, "provider")?;
    match provider {
        "mem0" => crate::provider_adapter::import_provider_mem0(state, args),
        _ => import_provider_stub(state, args),
    }
}

/// First-release `import_provider` stub (carve-out): no provider
/// adapters ship yet, so the tool exists, never writes, and reports the
/// structured unsupported code. Credential parameters are accepted only to
/// validate masking and are never logged or written.
#[inline(never)]
fn import_provider_stub(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let _provider = req_str(args, "provider")?;
    Ok(json!({
        "ok": false,
        "code": "PROVIDER_IMPORT_UNSUPPORTED",
        "message": "provider ingest adapters are not available in this release; \
                    use JSON export/import bundles instead",
        "credentials_masked": masked_credentials(args),
        "writes_performed": 0,
        "bank": state.default_bank
    }))
}

/// Last-4-masked view of any accepted credential parameters (never plaintext).
fn masked_credentials(args: &Value) -> serde_json::Value {
    clio_config::secret::masked_clone(args.get("credentials").unwrap_or(&serde_json::Value::Null))
}

#[cfg(test)]
#[path = "portability_tools_tests.rs"]
mod portability_tools_tests;
