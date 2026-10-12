// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-operation pre-flight handlers of the
//! `batch` engine: scoring each recognized Core write operation without
//! writing, and staging its row-level effects for the atomic executor.
//!
//! ## Owns
//! - One pre-flight per recognized batch op (`store`, `triple_add`,
//!   `canonical_put`, `persona_put_stable`, `task_upsert`, `failure_record`,
//!   `graph_link`)
//! - The shared capture-only gated scoring helper
//! - Threading the earlier staged items into each op's neighbor signals, so a
//!   duplicate inside one batch is visible to the op that repeats it
//!
//! ## Does not own
//! - Batch orchestration, `$ref` resolution, or report shape (`batch_tools`)
//! - Gate or admission math; the transactional executor (`clio_store::BatchStore`)
//!
//! ## Boundary
//!
//! Changes to a pre-flight's staged effects should normally require changes
//! here. Changes to batch orchestration should not require changes here.
//!
//! Keep this module focused on batch pre-flight staging.

use crate::batch_tools::Staged;
use crate::runtime::{BatchSignals, McpState, StderrSinks};
use crate::write_tools::{
    build_candidate, build_item, gated_write_policy, opt_context, opt_num, opt_str, req_str,
};
use clio_admission::gated::SignalSource;
use clio_store::{AssocEdgeRecord, AssocKind, BatchWrite};
use clio_types::{AmError, EpistemicKind, ErrorCode, MemoryItem, OpContext, SemanticCategory};
use serde_json::Value;
use std::sync::Arc;

/// Score and stage ONE operation without writing anything durable.
///
/// `staged` carries the items produced by the operations before this one, so a
/// duplicate inside the same batch is rejected the same way a duplicate already
/// in the bank is.
pub(crate) fn preflight(
    state: &McpState,
    tool: &str,
    params: &Value,
    in_batch_ids: &[String],
    staged: &[MemoryItem],
) -> Result<Staged, AmError> {
    let ctx = state.resolve_ctx(params)?;
    let ts = state.resolve_timestamp(params);
    let signals = BatchSignals::new(Arc::clone(&state.store), Arc::clone(&state.index), staged);
    match tool {
        "store" => store_preflight(state, &ctx, &ts, params, &signals),
        "canonical_put" => canonical_preflight(state, &ctx, &ts, params, &signals),
        "triple_add" => triple_preflight(state, &ctx, &ts, params, &signals),
        "persona_put_stable" | "task_upsert" | "failure_record" => {
            crate::batch_preflight_records::records_preflight(
                state, tool, &ctx, &ts, params, &signals,
            )
        }
        "graph_link" => graph_link_preflight(state, &ctx, params, in_batch_ids),
        _ => Err(AmError::new(
            ErrorCode::Internal,
            format!("unreachable batch tool `{tool}`"),
        )),
    }
}

/// Turn a structured admission rejection into a batch abort.
pub(crate) fn admission_rejected(decision: &clio_admission::decision::Decision) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        decision
            .rejection_reason
            .clone()
            .unwrap_or_else(|| "admission rejected".into()),
    )
}

/// Shared gated scoring: run both gates, capture the admitted item, never write.
fn score_item(
    state: &McpState,
    ctx: &OpContext,
    ts: &str,
    item: MemoryItem,
    params: &Value,
    candidate: clio_admission::decision::Candidate,
    source: &dyn SignalSource,
) -> Result<(MemoryItem, f64), AmError> {
    let fields = clio_write::default_event_fields();
    let mut req = clio_write::VerifiedStoreRequest {
        item,
        ctx,
        source_text: opt_str(params, "source_text"),
        evidence_ref: opt_str(params, "evidence_ref"),
        fields: &fields,
        verify: clio_write::VerifyConfig::default(),
        policy: gated_write_policy(state, ctx.bank.as_str(), params)?,
        admit_sink: &StderrSinks,
        verify_sink: &StderrSinks,
        timestamp: ts,
    };
    let decision = clio_write::gated_store_verified(&mut req, candidate, source, &mut |_| Ok(()))?;
    if !decision.pass {
        return Err(admission_rejected(&decision));
    }
    Ok((req.item, decision.admission_score))
}

fn store_preflight(
    state: &McpState,
    ctx: &OpContext,
    ts: &str,
    params: &Value,
    source: &dyn SignalSource,
) -> Result<Staged, AmError> {
    let candidate = build_candidate(params)?;
    let item_id = params
        .pointer("/item/id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map_or_else(|| state.next_item_id(), str::to_owned);
    let item = build_item(state, ctx, params, item_id)?;
    let (item, score) = score_item(state, ctx, ts, item, params, candidate, source)?;
    let id = item.id.clone();
    Ok(Staged {
        writes: vec![BatchWrite::CreateItem {
            item,
            actor: actor_of(ctx),
        }],
        id: Some(id),
        admission_score: Some(score),
    })
}

fn canonical_preflight(
    state: &McpState,
    ctx: &OpContext,
    ts: &str,
    params: &Value,
    source: &dyn SignalSource,
) -> Result<Staged, AmError> {
    let key = req_str(params, "key")?;
    let value = req_str(params, "value")?;
    let category = SemanticCategory::parse(req_str(params, "category")?)?;
    let epistemic_kind = EpistemicKind::parse(req_str(params, "epistemic_kind")?)?;
    let subject = clio_compliance::canonical_subject(key);
    let item_id = params
        .pointer("/item/id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map_or_else(|| state.next_item_id(), str::to_owned);
    let item = MemoryItem {
        id: item_id,
        bank_id: ctx.bank.as_str().to_owned(),
        subject_id: subject,
        kind: clio_types::ItemKind::Semantic,
        category: Some(category),
        episodic_type: None,
        epistemic_kind,
        source_type: opt_str(params, "source_type")
            .map(clio_types::SourceType::parse)
            .transpose()?,
        confidence: opt_num(params, "confidence"),
        admission_score: None,
        source_ref: None,
        snapshot: params.get("snapshot").cloned().filter(|v| !v.is_null()),
        gist: Some(value.to_owned()),
        valid_from: ts.to_owned(),
        valid_until: None,
        tx_from: ts.to_owned(),
        tx_until: None,
        created_at: ts.to_owned(),
        updated_at: ts.to_owned(),
        context: opt_context(params, "context")?,
    };
    let (mut item, score) = score_item(
        state,
        ctx,
        ts,
        item,
        params,
        clio_admission::decision::Candidate::Semantic(category),
        source,
    )?;
    item.admission_score = Some(score);
    // The prior slot row is resolved at commit time inside the batch
    // transaction (`BatchWrite::CanonicalPut`), so two puts on one key in one
    // batch still leave exactly one active revision.
    Ok(Staged {
        writes: vec![BatchWrite::CanonicalPut {
            item: item.clone(),
            key: key.to_owned(),
            actor: actor_of(ctx),
        }],
        id: Some(item.id),
        admission_score: Some(score),
    })
}

fn triple_preflight(
    state: &McpState,
    ctx: &OpContext,
    ts: &str,
    params: &Value,
    source: &dyn SignalSource,
) -> Result<Staged, AmError> {
    let subject = req_str(params, "subject")?;
    let context = opt_context(params, "context")?;
    let update_rule = opt_str(params, "update_rule")
        .map(clio_types::UpdateRule::parse)
        .transpose()?;
    let t_args = clio_write::triple::TripleAddArgs {
        ctx,
        subject,
        predicate: req_str(params, "predicate")?,
        object: req_str(params, "object")?,
        epistemic_kind: EpistemicKind::parse(req_str(params, "epistemic_kind")?)?,
        context: context.as_deref(),
        source_type: opt_str(params, "source_type")
            .map(clio_types::SourceType::parse)
            .transpose()?,
        confidence: opt_num(params, "confidence"),
        valid_from: opt_str(params, "valid_from"),
        valid_until: opt_str(params, "valid_until"),
        supersede: params
            .get("supersede")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        snapshot_ref: opt_str(params, "snapshot_ref"),
        gist_ref: opt_str(params, "gist_ref"),
        update_rule,
        subject_id: opt_str(params, "subject_id").unwrap_or(subject),
        policy: gated_write_policy(state, ctx.bank.as_str(), params)?,
        admit_sink: &StderrSinks,
        triple_sink: &StderrSinks,
        timestamp: ts,
        dry_run: true,
    };
    let r = clio_write::triple::triple_add(&*state.store, source, &t_args)?;
    if !r.decision.pass {
        return Err(admission_rejected(&r.decision));
    }
    let payload = r
        .payload
        .expect("triple pre-flight staged no payload when admitted");
    let item_id = payload.item.id.clone();
    Ok(Staged {
        writes: vec![BatchWrite::CommitTriple {
            item: payload.item,
            edge: payload.edge,
            supersede: payload.supersede,
            actor: actor_of(ctx),
        }],
        id: Some(item_id),
        admission_score: Some(r.decision.admission_score),
    })
}

fn graph_link_preflight(
    state: &McpState,
    ctx: &OpContext,
    params: &Value,
    in_batch_ids: &[String],
) -> Result<Staged, AmError> {
    let source = req_str(params, "source_id")?;
    let target = req_str(params, "target_id")?;
    let relationship = req_str(params, "relationship")?;
    let w = opt_num(params, "weight").unwrap_or(1.0);
    if source == target {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "graph_link requires two distinct item ids",
        ));
    }
    if !(0.0..=1.0).contains(&w) {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            "graph_link weight must be in [0, 1]",
        ));
    }
    // Endpoint existence: ids produced earlier in the SAME batch resolve at
    // commit time, so their rows are not queryable yet. Out-of-batch ids are
    // checked here to fail closed early.
    for id in [source, target] {
        if !in_batch_ids.iter().any(|staged| staged == id)
            && state
                .store
                .get_item_identity(ctx.bank.as_str(), id)?
                .is_none()
        {
            return Err(AmError::new(
                ErrorCode::ItemNotFound,
                format!("graph_link source/target `{id}` not found in bank"),
            ));
        }
    }
    let edge = AssocEdgeRecord {
        id: clio_store::explicit_id(ctx.bank.as_str(), source, target, relationship),
        bank_id: ctx.bank.as_str().to_owned(),
        source_id: source.to_owned(),
        target_id: target.to_owned(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some(relationship.to_owned()),
        weight: w,
        last_updated_at: state.resolve_timestamp(params),
    };
    Ok(Staged {
        writes: vec![
            BatchWrite::PutAssocEdge {
                edge,
                actor: actor_of(ctx),
            },
            BatchWrite::AuditEvent {
                bank_id: ctx.bank.as_str().to_owned(),
                operation: "graph_link".into(),
                item_id: Some(target.to_owned()),
                actor: actor_of(ctx),
                detail: serde_json::json!({"source_id": source, "relationship": relationship}),
            },
        ],
        id: None,
        admission_score: None,
    })
}

fn actor_of(ctx: &OpContext) -> String {
    ctx.actor.as_str().to_owned()
}
