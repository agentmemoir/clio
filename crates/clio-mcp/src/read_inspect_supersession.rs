// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-only supersession section of the
//! `inspect` surface: for one named item, it resolves the item's correction
//! pair from the lineage `audit_trail` returns and reports how that pair ranks
//! against the caller's query on one retrieval page.
//!
//! ## Owns
//! - The `supersession` report shape: verdict, pair ids, page positions,
//!   page scores, page counts, the methods reached, and the rerank flag
//! - The pair rule over the audit trail's revisions: `corrects_item_id`
//!   makes the queried item the replacement, `superseded_by_id` makes it the
//!   superseded one, and neither means no supersession link
//! - The not-on-page outcome: which of the two items is missing from the page
//!   and whether the page can tell absence from its cut
//! - The pure ordering verdict over one page's hits: positions and scores of
//!   the pair ids decide, with no store involved
//! - The required-`query` check when an `item_id` is present
//!
//! ## Does not own
//! - Lineage traversal (`clio-store` audit reads, reached through
//!   `clio_compliance::audit_trail`; no lineage walk of its own)
//! - Retrieval and ranking (`clio-retrieve`, reached through
//!   `state.diagnostic_retriever.retrieve`, the read-only diagnostic twin, so
//!   the section performs no durable reinforcement; read binding helpers come
//!   from `read_retrieve`/`read_read_policy`)
//! - The `inspect` listing itself and its payload fields (`read_tools`)
//!
//! ## Boundary
//!
//! Changes to the supersession report shape or the pair rule should normally
//! require changes here. Changes to retrieval binding helpers belong in
//! `read_retrieve`/`read_read_policy`; changes to ranking algorithms should
//! not require changes here.
//!
//! Keep this module focused on the inspect supersession section.

use super::{opt_usize, req_str};
use crate::read_read_policy::read_provenance_floor;
use crate::read_retrieve::{read_curve, read_policy};
use crate::runtime::McpState;
use clio_compliance::AuditTrailView;
use clio_retrieve::{DEFAULT_LIMIT, RetrieveOutcome, RetrieveRequest, ScoredHit};
use clio_types::{AmError, OpContext};
use serde_json::{Value, json};

/// Which side of a supersession pair ranks where on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The replacement hit precedes the superseded hit on the page.
    ReplacementFirst,
    /// The superseded hit precedes the replacement hit on the page.
    SupersededFirst,
    /// One or both pair members are missing from the page.
    NotOnPage,
    /// The queried item's lineage carries no supersession link.
    NoLink,
}

impl Verdict {
    /// The stable report-surface label.
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::ReplacementFirst => "replacement_first",
            Self::SupersededFirst => "superseded_first",
            Self::NotOnPage => "not_on_page",
            Self::NoLink => "no_link",
        }
    }
}

/// The supersession pair the audit trail names for the queried item.
struct Pair {
    replacement_id: String,
    superseded_id: String,
}

/// Build the `supersession` section for one item, using the caller's query.
///
/// Read-only on every path: one `audit_trail` read resolves the pair, and a
/// linked pair runs exactly one retrieval with the caller's query.
pub(super) fn section(
    state: &McpState,
    ctx: &OpContext,
    args: &Value,
    item_id: &str,
) -> Result<Value, AmError> {
    let query = req_str(args, "query")?;
    let view = clio_compliance::audit_trail(&*state.store, ctx.bank.as_str(), item_id)?;
    let Some(pair) = pair_from_trail(&view, item_id) else {
        return Ok(no_link(item_id));
    };
    let (limit, outcome) = retrieve_page(state, ctx, args, query)?;
    Ok(page_report(&pair, item_id, limit, &outcome))
}

/// The pair rule over the audit trail's revisions.
///
/// The revision row of the queried item decides: `corrects_item_id` names the
/// prior assertion the queried item replaces; otherwise `superseded_by_id`
/// names the item that replaces the queried one; otherwise there is no link.
fn pair_from_trail(view: &AuditTrailView, item_id: &str) -> Option<Pair> {
    let revision = view
        .revisions
        .iter()
        .find(|r| str_field(r, "item_id") == item_id)?;
    let corrects = str_field(revision, "corrects_item_id");
    if !corrects.is_empty() {
        return Some(Pair {
            replacement_id: item_id.to_owned(),
            superseded_id: corrects,
        });
    }
    let superseded_by = str_field(revision, "superseded_by_id");
    if !superseded_by.is_empty() {
        return Some(Pair {
            replacement_id: superseded_by,
            superseded_id: item_id.to_owned(),
        });
    }
    None
}

/// One retrieval page with the caller's query, bound exactly as the
/// `retrieve` tool binds its read-side policy.
fn retrieve_page(
    state: &McpState,
    ctx: &OpContext,
    args: &Value,
    query: &str,
) -> Result<(usize, RetrieveOutcome), AmError> {
    let request = page_request(state, ctx, args, query)?;
    let limit = request.limit;
    let outcome = state.diagnostic_retriever.retrieve(&request)?;
    Ok((limit, outcome))
}

/// The request the section binds: the caller's query with the read-side policy
/// resolved exactly as the `retrieve` tool resolves it. Split from
/// [`retrieve_page`] so the twin-equivalence test can run the same request
/// through both retrievers and compare the pages.
fn page_request(
    state: &McpState,
    ctx: &OpContext,
    args: &Value,
    query: &str,
) -> Result<RetrieveRequest, AmError> {
    let (recall_scope, duplicate_tolerance_read) = read_policy(state, ctx, args)?;
    let (retention_curve, retention_floor) = read_curve(state, ctx);
    let provenance_floor = read_provenance_floor(state, ctx.bank.as_str(), None)?;
    let limit = opt_usize(args, "limit").unwrap_or(DEFAULT_LIMIT);
    Ok(RetrieveRequest {
        bank: ctx.bank.as_str().to_owned(),
        query: query.to_owned(),
        limit,
        recall_scope,
        duplicate_tolerance_read,
        retention_curve,
        retention_floor,
        provenance_floor,
        ..RetrieveRequest::new("", "")
    })
}

/// One page hit reduced to the fields the ordering verdict reads: the item id
/// and its fused page score.
struct PageHit {
    item_id: String,
    score: f64,
}

impl PageHit {
    fn of(hit: &ScoredHit) -> Self {
        Self {
            item_id: hit.hit.item_id.clone(),
            score: hit.score,
        }
    }
}

/// The pure ordering outcome for one pair over one page.
#[derive(Debug)]
struct Ordering {
    verdict: Verdict,
    /// The replacement's position and score when it is on the page.
    replacement: Option<(usize, f64)>,
    /// The superseded member's position and score when it is on the page.
    superseded: Option<(usize, f64)>,
    /// Which pair members are missing from the page.
    missing: Vec<&'static str>,
}

/// The ordering verdict over one page: pure over the hits (ids plus scores)
/// and the pair ids, so the two orderings and the not-on-page outcome are
/// unit-testable with synthetic in-memory pages.
fn ordering(hits: &[PageHit], replacement_id: &str, superseded_id: &str) -> Ordering {
    let position = |id: &str| hits.iter().position(|h| h.item_id == id);
    let replacement = position(replacement_id);
    let superseded = position(superseded_id);
    let verdict = match (replacement, superseded) {
        (Some(r), Some(s)) if r < s => Verdict::ReplacementFirst,
        (Some(_), Some(_)) => Verdict::SupersededFirst,
        _ => Verdict::NotOnPage,
    };
    let mut missing = Vec::new();
    if replacement.is_none() {
        missing.push("replacement");
    }
    if superseded.is_none() {
        missing.push("superseded");
    }
    Ordering {
        verdict,
        replacement: replacement.map(|i| (i, hits[i].score)),
        superseded: superseded.map(|i| (i, hits[i].score)),
        missing,
    }
}

/// The report for a linked pair, from one retrieval page.
///
/// Carries ids, positions, and scores only — never gist, context, snapshot,
/// or any other content field.
fn page_report(pair: &Pair, item_id: &str, limit: usize, outcome: &RetrieveOutcome) -> Value {
    let hits: Vec<PageHit> = outcome.hits.iter().map(PageHit::of).collect();
    let ord = ordering(&hits, &pair.replacement_id, &pair.superseded_id);
    let mut report = serde_json::Map::new();
    report.insert("verdict".into(), json!(ord.verdict.as_str()));
    report.insert("query_item_id".into(), json!(item_id));
    report.insert("replacement_item_id".into(), json!(pair.replacement_id));
    report.insert("superseded_item_id".into(), json!(pair.superseded_id));
    if let (Some((r, r_score)), Some((s, s_score))) = (ord.replacement, ord.superseded) {
        report.insert("replacement_position".into(), json!(r));
        report.insert("superseded_position".into(), json!(s));
        report.insert("replacement_score".into(), json!(r_score));
        report.insert("superseded_score".into(), json!(s_score));
    } else {
        report.insert("missing".into(), json!(ord.missing));
        report.insert("reason".into(), json!(page_reason(outcome, limit)));
    }
    report.insert("page_size".into(), json!(outcome.hits.len()));
    report.insert("page_limit".into(), json!(limit));
    report.insert("reads".into(), json!(["audit_trail", "retrieve"]));
    report.insert("reranked".into(), json!(outcome.reranked));
    Value::Object(report)
}

/// Why a pair member is missing from the page: a short page proves absence;
/// a full page cannot be distinguished from the cut.
fn page_reason(outcome: &RetrieveOutcome, limit: usize) -> String {
    if outcome.hits.len() < limit {
        "the page returned fewer hits than the requested limit, so the missing item is absent from the page".to_owned()
    } else {
        "the page is full (hits equal the requested limit), so it cannot be distinguished whether the missing item is absent or below the page cut".to_owned()
    }
}

/// The report for a lineage with no supersession link: a distinct outcome,
/// not a failing case, and no retrieval runs.
fn no_link(item_id: &str) -> Value {
    json!({
        "verdict": Verdict::NoLink.as_str(),
        "query_item_id": item_id,
        "page_size": 0,
        "page_limit": 0,
        "reads": ["audit_trail"]
    })
}

/// String field of a JSON object (empty when absent/non-string).
fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
#[path = "read_inspect_supersession_guard_tests.rs"]
mod read_inspect_supersession_guard_tests;
#[cfg(test)]
#[path = "read_inspect_supersession_tests.rs"]
mod read_inspect_supersession_tests;
