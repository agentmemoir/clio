// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for external memory provider ingest orchestration
//! (`import_provider`), pagination, credential masking, gating, and
//! external-id idempotency.
//!
//! ## Owns
//! - Mem0 REST API pull client (`/v1/memories/`) with pagination and auth headers
//! - Provider item mapping: `provider:<id>` provenance, `belief`/`third_party` defaults
//! - Dry-run reporting (zero writes) matching JSON `ImportReport`
//! - External-id idempotency (repeat pulls skip already-admitted items)
//! - In-batch deduplication parity: repeated external ids report `would_skip`
//!   in dry runs exactly as the live path skips them after the first write
//!
//! ## Does not own
//! - MCP argument decoding or routing (`portability_tools.rs`)
//! - Underlying HTTP transport (`clio-index::get_json`)
//! - Gated admission decision logic (`clio-admission`)
//! - Durable storage SQL (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to provider ingest mapping, pagination, or provider adapter logic
//! should normally require changes here. Changes to MCP tool schema or transport
//! should not require changes here.
//!
//! Keep this module focused on provider memory ingest.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use clio_admission::AdmissionPolicy;
use clio_admission::gated::{GateRequest, gated_create_raw};
use clio_compliance::ImportReport;
use clio_config::secret::{mask_secret, masked_clone, scrub_inline_secrets};
use clio_types::{
    AmError, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, SemanticCategory,
    SourceType,
};
use serde_json::{Value, json};

use crate::runtime::{McpState, StderrSinks, StoreSignals};
use crate::write_tools::{gated_write_policy, opt_bool, opt_str};

const DEFAULT_MEM0_BASE_URL: &str = "https://api.mem0.ai/v1";
const DEFAULT_PAGE_SIZE: u64 = 50;
const DEFAULT_MAX_ITEMS: u64 = 500;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

enum ItemOutcome {
    Created,
    Skipped,
    Rejected(Option<String>),
}

/// Import memories from Mem0 end-to-end: pull, credential masking, dry-run
/// reporting, gating, `provider:<id>` provenance, and external-id idempotency.
pub(crate) fn import_provider_mem0(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let ctx = state.resolve_ctx(args)?;
    let ts = state.resolve_timestamp(args)?;
    let dry_run = opt_bool(args, "dry_run", false)?;
    let options = args.get("options").unwrap_or(&Value::Null);

    let base_url = opt_str(options, "api_base_url")
        .or_else(|| opt_str(options, "base_url"))
        .unwrap_or(DEFAULT_MEM0_BASE_URL);
    let user_id = opt_str(options, "user_id");
    let agent_id = opt_str(options, "agent_id");
    let page_size = options
        .get("page_size")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .max(1);
    let max_items = options
        .get("max_items")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_ITEMS)
        .max(1);

    let api_key = extract_api_key(args);
    let auth_header = api_key.as_deref().map(|k| {
        if k.starts_with("Token ") || k.starts_with("Bearer ") {
            k.to_owned()
        } else {
            format!("Token {k}")
        }
    });

    let policy = gated_write_policy(state, ctx.bank.as_str(), args)?;
    let signals = StoreSignals::new(Arc::clone(&state.store), Arc::clone(&state.index));

    let mut would_create = 0usize;
    let mut would_skip = 0usize;
    let mut would_reject = 0usize;
    let mut rejection_samples = Vec::new();
    let mut notes = Vec::new();

    let mut page = 1u64;
    let mut total_seen = 0u64;
    let mut seen_external_ids: HashSet<String> = HashSet::new();

    loop {
        let url = build_mem0_url(base_url, page, page_size, user_id, agent_id);
        let resp = fetch_with_retry(&url, auth_header.as_deref())?;
        let items = parse_mem0_items(&resp)?;
        if items.is_empty() {
            break;
        }

        let page_count = items.len() as u64;
        for raw in &items {
            if total_seen >= max_items {
                notes.push(format!("reached max_items cap of {max_items}"));
                break;
            }
            total_seen += 1;

            match process_mem0_item(
                state,
                raw,
                options,
                &ctx,
                &ts,
                &policy,
                &signals,
                dry_run,
                &mut seen_external_ids,
            )? {
                ItemOutcome::Created => would_create += 1,
                ItemOutcome::Skipped => would_skip += 1,
                ItemOutcome::Rejected(reason) => {
                    would_reject += 1;
                    if let Some(r) = reason
                        && rejection_samples.len() < 10
                    {
                        rejection_samples.push(scrub_inline_secrets(&r));
                    }
                }
            }
        }

        if total_seen >= max_items || page_count < page_size {
            break;
        }
        page += 1;
    }

    let report = ImportReport {
        would_create,
        would_skip,
        would_reject,
        would_overwrite: 0,
        // Provider rows are scored item by item, so no entity is skipped for
        // invalid stored content here.
        would_skip_invalid: 0,
        skipped_invalid: Vec::new(),
        // The provider adapter is reached through MCP only, never through the
        // one-shot CLI drain, so it reports no per-item ids.
        created_ids: Vec::new(),
        rejection_samples,
        dry_run,
        applied: !dry_run,
        notes,
    };

    Ok(json!({
        "ok": true,
        "provider": "mem0",
        "credentials_masked": masked_credentials(args),
        "report": serde_json::to_value(&report).expect("import report serializes"),
        "writes_performed": if dry_run { 0 } else { would_create },
        "bank": ctx.bank.as_str()
    }))
}

#[allow(clippy::too_many_arguments)]
fn process_mem0_item(
    state: &McpState,
    raw: &Value,
    options: &Value,
    ctx: &OpContext,
    ts: &str,
    policy: &AdmissionPolicy<'_>,
    signals: &StoreSignals,
    dry_run: bool,
    seen_external_ids: &mut HashSet<String>,
) -> Result<ItemOutcome, AmError> {
    let ext_id = match raw.get("id").and_then(Value::as_str) {
        Some(id) if !id.trim().is_empty() => id.trim(),
        _ => {
            return Ok(ItemOutcome::Rejected(Some(
                "missing external item id".into(),
            )));
        }
    };

    let item_id = format!("item-mem0-{ext_id}");
    if seen_external_ids.contains(ext_id)
        || state
            .store
            .get_memory_item(ctx.bank.as_str(), &item_id)?
            .is_some()
    {
        return Ok(ItemOutcome::Skipped);
    }

    let text = raw
        .get("memory")
        .or_else(|| raw.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return Ok(ItemOutcome::Rejected(Some(
            "empty memory text content".into(),
        )));
    }

    let cat_str = determine_category(raw, options);
    let category = SemanticCategory::parse(&cat_str).ok();
    let user_id = raw.get("user_id").and_then(Value::as_str);
    let subject_id = user_id.map_or_else(
        || format!("subj-{}", ctx.bank.as_str()),
        |u| format!("subj-mem0-{u}"),
    );

    let item = MemoryItem {
        id: item_id,
        bank_id: ctx.bank.as_str().to_owned(),
        subject_id,
        kind: ItemKind::Semantic,
        category,
        episodic_type: None,
        epistemic_kind: EpistemicKind::Belief,
        source_type: Some(SourceType::ThirdParty),
        confidence: raw.get("confidence").and_then(Value::as_f64),
        admission_score: None,
        source_ref: Some(format!("provider:{ext_id}")),
        snapshot: Some(json!({
            "memory": text,
            "user_id": user_id,
            "categories": raw.get("categories"),
            "metadata": raw.get("metadata"),
        })),
        gist: Some(text.to_owned()),
        valid_from: ts.to_owned(),
        valid_until: None,
        tx_from: ts.to_owned(),
        tx_until: None,
        created_at: ts.to_owned(),
        updated_at: ts.to_owned(),
        context: None,
    };

    let decision = gated_create_raw(
        &GateRequest {
            item: &item,
            ctx,
            policy: policy.clone(),
            sink: &StderrSinks,
            timestamp: ts,
        },
        "semantic",
        &cat_str,
        signals,
        &mut |admitted: &MemoryItem| {
            if !dry_run {
                state
                    .store
                    .create_memory_item(admitted, ctx.actor.as_str())?;
            }
            Ok(())
        },
    )?;

    if decision.pass {
        seen_external_ids.insert(ext_id.to_owned());
        Ok(ItemOutcome::Created)
    } else {
        Ok(ItemOutcome::Rejected(decision.rejection_reason))
    }
}

/// Last-4-masked view of accepted credentials (never plaintext).
pub(crate) fn masked_credentials(args: &Value) -> Value {
    masked_credentials_with(args, &|k| std::env::var(k).ok())
}

pub(crate) fn masked_credentials_with(args: &Value, env: &dyn Fn(&str) -> Option<String>) -> Value {
    if let Some(creds) = args.get("credentials") {
        match creds {
            Value::String(s) => Value::String(mask_secret(s)),
            Value::Object(_) => masked_clone(creds),
            Value::Null => Value::Null,
            other => masked_clone(other),
        }
    } else if let Some(env_key) = env("CLIO_MEM0_API_KEY") {
        json!({ "api_key": mask_secret(&env_key) })
    } else {
        Value::Null
    }
}

fn extract_api_key(args: &Value) -> Option<String> {
    extract_api_key_with(args, &|k| std::env::var(k).ok())
}

fn extract_api_key_with(args: &Value, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if let Some(creds) = args.get("credentials") {
        if let Some(s) = creds.as_str()
            && !s.trim().is_empty()
        {
            return Some(s.trim().to_owned());
        }
        if let Some(obj) = creds.as_object() {
            for key in ["api_key", "token", "key", "secret"] {
                if let Some(val) = obj.get(key).and_then(Value::as_str)
                    && !val.trim().is_empty()
                {
                    return Some(val.trim().to_owned());
                }
            }
        }
    }
    env("CLIO_MEM0_API_KEY").filter(|s| !s.trim().is_empty())
}

fn build_mem0_url(
    base: &str,
    page: u64,
    page_size: u64,
    user_id: Option<&str>,
    agent_id: Option<&str>,
) -> String {
    let trimmed = base.trim_end_matches('/');
    let path = if trimmed.ends_with("/v1") {
        format!("{trimmed}/memories/")
    } else if trimmed.contains("/memories") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1/memories/")
    };
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut url = format!("{path}{sep}page={page}&page_size={page_size}");
    if let Some(u) = user_id {
        url.push_str("&user_id=");
        url.push_str(u);
    }
    if let Some(a) = agent_id {
        url.push_str("&agent_id=");
        url.push_str(a);
    }
    url
}

fn fetch_with_retry(url: &str, auth_header: Option<&str>) -> Result<Value, AmError> {
    let mut attempts = 0u64;
    loop {
        attempts += 1;
        match clio_index::get_json(url, HTTP_TIMEOUT, auth_header) {
            Ok(val) => return Ok(val),
            Err(err) if attempts < 3 && is_retryable_error(&err) => {
                std::thread::sleep(Duration::from_millis(50 * attempts));
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_retryable_error(err: &AmError) -> bool {
    let text = err.to_string();
    text.contains("HTTP 429")
        || text.contains("HTTP 503")
        || text.contains("sidecar unreachable")
        || text.contains("timed out")
        || text.contains("connect failed")
}

fn parse_mem0_items(resp: &Value) -> Result<Vec<Value>, AmError> {
    if let Some(arr) = resp.as_array() {
        return Ok(arr.clone());
    }
    if let Some(obj) = resp.as_object() {
        for key in ["results", "data", "memories", "items"] {
            if let Some(arr) = obj.get(key).and_then(Value::as_array) {
                return Ok(arr.clone());
            }
        }
    }
    Err(AmError::new(
        ErrorCode::Internal,
        "invalid provider response: expected JSON array of memories",
    ))
}

fn determine_category(raw: &Value, options: &Value) -> String {
    if let Some(cat) = opt_str(options, "category") {
        return cat.to_owned();
    }
    if let Some(cat) = opt_str(raw, "category") {
        return cat.to_owned();
    }
    if let Some(cats) = raw.get("categories").and_then(Value::as_array) {
        for c in cats {
            if let Some(s) = c.as_str() {
                let lower = s.to_ascii_lowercase();
                if lower.contains("task") {
                    return "task_spec".to_owned();
                }
                if lower.contains("schema") {
                    return "schema".to_owned();
                }
                if lower.contains("tool") {
                    return "tool_config".to_owned();
                }
                if lower.contains("constraint") {
                    return "output_constraint".to_owned();
                }
                if lower.contains("persona")
                    || lower.contains("preference")
                    || lower.contains("profile")
                {
                    return "persona".to_owned();
                }
            }
        }
    }
    "persona".to_owned()
}

#[cfg(test)]
#[path = "provider_adapter_tests.rs"]
mod provider_adapter_tests;

#[cfg(test)]
#[path = "provider_adapter_edge_tests.rs"]
mod provider_adapter_edge_tests;
