// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for measuring the rendered-size distribution of
//! both snapshot producers against the exact-channel cap: one fixture per item id
//! shape this repository's write paths mint, and one per model output the
//! template extractor admits.
//!
//! ## Owns
//! - `the_hub_distilled_median_fits_the_cap_at_the_default_neighbour_count`
//! - `the_template_extracted_distribution_is_reported`
//! - `a_rendering_is_the_whole_projection_or_nothing_at_all`
//!
//! ## Does not own
//! - The cap and the rendering rule (`clio-retrieve` `render`)
//! - The snapshot projection (`clio-store`) and the extractor (`clio-write`)
//! - What a channel does with an unrenderable hit (`clio-mcp` `pack_forms`)
//!
//! ## Boundary
//!
//! Changes to what the write paths mint as item ids, or to what the extractor
//! admits, should normally require changes here. A change to the cap should not:
//! it moves the numbers this prints.

use clio_retrieve::{EXACT_RENDER_MAX_BYTES, render_exact};
use clio_store::{
    DistilledUnitRecord, STALE_SOURCE_CHANGED, STALE_SOURCE_REMOVED, revision_item_id,
    snapshot_projection,
};
use clio_types::AmError;
use clio_write::{
    DEFAULT_HUB_NEIGHBORS, ExtractRequest, Extractor, HUB_DISTILL_MARKER, TemplateApiExtractor,
    Transport,
};
use serde_json::{Value, json};

/// Bank for the hub fixtures. The first-distillation unit id is
/// `hub-distill:{bank}:{hub}`, so the bank name is inside every rendering.
const BANK: &str = "default";
/// A longer real bank name, to show what the bank name itself costs.
const LONG_BANK: &str = "team-alpha-platform";
/// Fixed stamp for every record and fixture; nothing here reads a clock.
const TS: &str = "2026-10-01T09:30:00Z";
/// A hub plus its claim-set sources at the default neighbour count.
const N: usize = DEFAULT_HUB_NEIGHBORS + 1;

// The item id shapes the write paths mint, each with that minter's own counter
// placeholder so no fixture below carries a made-up length.
const MCP: &str = "itm-mcp-{}";
const RAW: &str = "raw-0-{}";
const RAW_LONG: &str = "conversation-turn-0-{}";
const TRIPLE_WIDE: &str = "itm-2026-10-01T09:30:00Z-1757067896123456789-{}-4242";
const TRIPLE_SMALL: &str = "itm-2026-10-01T09:30:00Z-1757067896123456789-{}-742";
const PROVIDER: &str = "item-mem0-01J8Z9KQ3M{:04X}";
const UUID: &str = "{:08x}-3f7a-4c1e-9b2d-a1b2c3d4e5f6";
const SLUG: &str = "episodic/2026-10-01/planner-review-{}";

/// Count, extremes, median, and the two cap fractions over one fixture bank.
#[derive(Debug)]
struct Distribution {
    count: usize,
    min: usize,
    median: usize,
    max: usize,
    within_cap: usize,
    rendered: usize,
}

/// Render every snapshot in a bank, summarise the lengths, and print the whole
/// distribution as run evidence, not only assertions. The median is the middle
/// value, the two middles of an even count averaged down.
fn measure(name: &str, bank: &[(&'static str, Value)]) -> Distribution {
    let cap = EXACT_RENDER_MAX_BYTES;
    let mut ordered: Vec<(usize, bool)> = Vec::with_capacity(bank.len());
    for (label, snapshot) in bank {
        // `render_exact` publishes the whole serialization or nothing, so the
        // length is the serialization's either way and the flag says which.
        let bytes = serde_json::to_string(snapshot).expect("serializes").len();
        let rendered = render_exact(snapshot).is_some();
        assert_eq!(
            rendered,
            bytes <= cap,
            "{name}: `{label}` fits the cap iff rendered"
        );
        ordered.push((bytes, rendered));
    }
    assert!(!ordered.is_empty(), "{name} bank is empty");
    let mut lengths: Vec<usize> = ordered.iter().map(|(bytes, _)| *bytes).collect();
    lengths.sort_unstable();
    let mid = lengths.len() / 2;
    let median = if lengths.len().is_multiple_of(2) {
        usize::midpoint(lengths[mid - 1], lengths[mid])
    } else {
        lengths[mid]
    };
    let dist = Distribution {
        count: lengths.len(),
        min: lengths[0],
        median,
        max: lengths[lengths.len() - 1],
        within_cap: lengths.iter().filter(|n| **n <= cap).count(),
        rendered: ordered.iter().filter(|(_, hit)| *hit).count(),
    };
    eprintln!("=== {name}: {dist:?} cap={cap} ===");
    for ((label, _), (bytes, hit)) in bank.iter().zip(&ordered) {
        let verdict = if *hit { "rendered" } else { "dropped" };
        eprintln!("    {label:<44} {bytes:>4} bytes  {verdict}");
    }
    eprintln!(
        "    within_cap={}/{} exact_hit_rate={}/{}",
        dist.within_cap, dist.count, dist.rendered, dist.count
    );
    dist
}

/// Ids one of this repository's write paths mints: one id per sequence number,
/// built from that minter's own format and counter placeholder. `shape` carries
/// the counter placeholder `{}` for a decimal one, `{:04X}` for an uppercase-hex
/// one, and `{:08x}` for a lowercase-hex one.
fn ids(shape: &str, from: usize, count: usize) -> Vec<String> {
    (from..from + count)
        .map(|i| {
            shape
                .replace("{:04X}", &format!("{i:04X}"))
                .replace("{:08x}", &format!("{i:08x}"))
                .replace("{}", &i.to_string())
        })
        .collect()
}

/// The record the distiller writes for one hub at first distillation.
///
/// `source` is the hub first, then its claim-set sources; the neighbour scan
/// takes at most `DEFAULT_HUB_NEIGHBORS` of them.
fn unit_record(bank: &str, source: &[String]) -> DistilledUnitRecord {
    let (hub, neighbours) = source.split_first().expect("a hub id");
    DistilledUnitRecord::new(
        bank,
        format!("hub-distill:{bank}:{hub}"),
        hub.clone(),
        neighbours.iter().take(DEFAULT_HUB_NEIGHBORS).cloned(),
        HUB_DISTILL_MARKER,
        TS,
    )
}

/// The projection of a real record, through the store's one projection function.
fn hub(bank: &str, shape: &str, from: usize, count: usize) -> Value {
    snapshot_projection(&unit_record(bank, &ids(shape, from, count)))
}

/// The projection of a unit a reconciliation revised to revision 2, whose unit id
/// then carries the revision suffix.
fn revised_hub(bank: &str, shape: &str) -> Value {
    let mut record = unit_record(bank, &ids(shape, 0, N));
    record.revision = 2;
    record.item_id = revision_item_id(&record.item_id, 2);
    snapshot_projection(&record)
}

/// The projection of a unit not current under `reason`, which adds the reason.
fn stale_hub(bank: &str, shape: &str, reason: &str) -> Value {
    let mut record = unit_record(bank, &ids(shape, 0, N));
    record.stale_reason = Some(reason.to_owned());
    record.stale_at = Some(TS.to_owned());
    snapshot_projection(&record)
}

/// One row of the hub bank: a label, the bank, the item id shape one write path
/// mints, where that minter's counter stands, and how many ids to mint.
const HUB_ROWS: &[(&str, &str, &str, usize, usize)] = &[
    // The MCP `store` path's id minter, used when a caller omits `item.id`: the
    // shipped default bank, that bank once its per-process counter advanced, and
    // the same minter in a longer bank name.
    ("mcp store, omitted item.id", BANK, MCP, 0, N),
    ("mcp store, counter advanced", BANK, MCP, 1042, N),
    ("mcp store, longer bank name", LONG_BANK, MCP, 0, N),
    // The raw-ingest path's caller-supplied `item_id_prefix`: the repository's
    // own `raw` prefix, and a longer prefix a host supplies.
    ("raw ingest, `raw` prefix", BANK, RAW, 0, N),
    ("raw ingest, longer prefix", BANK, RAW_LONG, 3, N),
    // The triple-add path's id minter, `{prefix}-{timestamp}-{nanos}-{seq}-{pid}`:
    // the timestamp and the nanosecond count are most of what makes these ids
    // long, and the process id's width is what varies between them.
    ("triple add, wide pid", BANK, TRIPLE_WIDE, 0, N),
    ("triple add, small pid", BANK, TRIPLE_SMALL, 0, N),
    // The provider import path's external id, then two hosts passing their own
    // `item.id` through the MCP `store` tool: a UUID and a readable path slug.
    ("provider import id", BANK, PROVIDER, 0, N),
    ("caller uuid ids", BANK, UUID, 0, N),
    ("caller path slug ids", BANK, SLUG, 0, N),
    // A hub whose neighbour scan returned fewer claimable sources than the
    // limit, which is a real outcome and a shorter projection.
    ("fewer claimable sources", BANK, MCP, 0, 4),
];

/// The hub-distilled fixture bank: one entry per item id shape this repository's
/// write paths mint, plus the snapshot variants that move the size without any id
/// changing. Every entry is at the default eight neighbours except the fixture that
/// deliberately holds a hub with fewer claimable sources. The projection carries
/// the record's own fields and nothing else, so at the default neighbour count the
/// rendering is `86 + |unit_id| + |hub_id| + sum(|neighbour_id|)` bytes, and the unit
/// id repeats the hub id behind `hub-distill:{bank}:`.
fn hub_bank() -> Vec<(&'static str, Value)> {
    let mut bank: Vec<(&'static str, Value)> = HUB_ROWS
        .iter()
        .map(|(label, bank, shape, from, count)| (*label, hub(bank, shape, *from, *count)))
        .collect();
    // A reconciled unit, and a unit not current under each reason a wave records.
    bank.extend([
        ("revision 2", revised_hub(BANK, MCP)),
        (
            "stale, source removed",
            stale_hub(BANK, MCP, STALE_SOURCE_REMOVED),
        ),
        (
            "stale, source changed",
            stale_hub(BANK, PROVIDER, STALE_SOURCE_CHANGED),
        ),
    ]);
    bank
}

/// Stub transport answering one extraction call with a canned model response,
/// so the real extractor runs with no network.
#[derive(Debug)]
struct CannedTransport {
    response: Value,
}

impl Transport for CannedTransport {
    fn post_json(&self, _url: &str, _body: &Value) -> Result<Value, AmError> {
        Ok(self.response.clone())
    }
}

/// The schema template a caller writes for one model answer: every key with an
/// empty placeholder per leaf, this repository's own template shape.
fn schema_template(answer: &Value) -> Value {
    fn blank(value: &Value) -> Value {
        match value {
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, v)| (k.clone(), blank(v))).collect())
            }
            Value::Array(items) => Value::Array(items.iter().map(blank).collect()),
            _ => Value::String(String::new()),
        }
    }
    let map = answer.as_object().expect("a model answer is an object");
    Value::Object(map.iter().map(|(k, v)| (k.clone(), blank(v))).collect())
}

/// Drive the real template extractor over one model answer and take the snapshot
/// it publishes. The stub stands in for the model only: the request body, the
/// rule that only a JSON object is a candidate, and the candidate itself are the
/// real extractor's.
fn extracted(model_answer: &Value) -> Value {
    let extractor = TemplateApiExtractor {
        url: "http://127.0.0.1:1/extract".to_owned(),
        model: "numind/NuExtract-1.5-tiny".to_owned(),
        temperature: 0.0,
        transport: CannedTransport {
            response: json!({"snapshot": model_answer, "source_ref": "turn_1"}),
        },
    };
    let request = ExtractRequest {
        source_text: "a recorded turn of prose".to_owned(),
        tool_payload: None,
        schema_template: schema_template(model_answer),
        verify_feedback: None,
        context: None,
    };
    let candidate = extractor.extract(&request).expect("the stub answers");
    candidate.snapshot.expect("a JSON object is a candidate")
}

/// The template-extracted fixture bank: one entry per model output shape a real
/// template admits.
///
/// This producer is **genuinely unbounded**: the extractor takes whatever
/// `snapshot` object the model returned and the write path never bounds it, so
/// the bank spans a realistic range of model outputs rather than one size.
fn template_bank() -> Vec<(&'static str, Value)> {
    let answers = vec![
        // The repository's one-field `{"entity": ""}` template and the
        // three-field event set its field spec requires, answered the way a
        // template model fills a placeholder.
        ("one-field template", json!({"entity": "backlog item"})),
        (
            "three-field event template",
            json!({"entity": "planner", "amount": 1, "date": "2024-01-01"}),
        ),
        // A template the recall tests carry, and the empty `{}` template several
        // call sites pass, which a model may answer with an empty object.
        (
            "rust and os template",
            json!({"rust_version": "1.78.0", "os": "linux"}),
        ),
        ("empty template answered empty", json!({})),
        // A model that returns keys the template never asked for, which nothing
        // downstream removes, and a field filled with a whole clause.
        (
            "keys the template never asked for",
            json!({"entity": "deploy-bot", "confidence": 0.82, "source_turn": "t-42"}),
        ),
        (
            "a field filled with a whole clause",
            json!({"entity": "the quarterly planning review that moved the launch date to the first week of November"}),
        ),
        // A long span copied out of the turn, and a value in a script where one
        // character occupies more than one byte.
        (
            "a long span copied out of the turn",
            json!({"entity": "we agreed to move the launch to the first week of november because the vendor could not ship the certification harness before the end of october and the compliance review needs a full week of soak time"}),
        ),
        (
            "a non-ascii value",
            json!({"entity": "四半期の計画レビューでリリース日を11月第一週に変更する。Globalization release note."}),
        ),
        // The summarize path, where the template *is* an existing snapshot and
        // the model echoes its shape, and the free-form provider shape.
        (
            "summarize: template is a stored snapshot",
            json!({
            "hub_id": "itm-mcp-0", "neighbor_count": DEFAULT_HUB_NEIGHBORS,
            "neighbor_ids": ids(MCP, 1, DEFAULT_HUB_NEIGHBORS),
            "unit_id": "hub-distill:default:itm-mcp-0"}),
        ),
        (
            "the free-form provider shape",
            json!({
            "memory": "user prefers short standup notes and wants decisions written down with an owner and a date",
            "user_id": "mem0-user-88213", "categories": ["preference", "workflow"],
            "metadata": {"source": "slack", "thread": "t-01924f", "reactions": 3}}),
        ),
        // A host's larger own templates, which are the tail: a nested incident
        // record that lands just under the cap, and a twelve-field meeting note
        // that does not fit it.
        (
            "a nested incident template",
            json!({
            "entity": "incident-4412", "service": "checkout-api", "severity": "sev2",
            "owner": "payments-oncall", "started_at": "2026-10-04T21:07:00Z",
            "resolved_at": "2026-10-04T23:41:00Z",
            "customer_impact": "elevated 502s on the checkout page for eu-west-1 customers",
            "root_cause": "a bad connection-pool setting shipped in the 21:02 deploy exhausted the primary pool",
            "remediation": "pool max raised to 64 and the deploy rolled forward",
            "tags": ["payments", "eu-west-1", "pool"]}),
        ),
        (
            "a twelve-field meeting template",
            json!({
            "title": "Weekly platform sync for the eu-west-1 migration programme",
            "attendees": ["infra-oncall", "payments-oncall", "data-platform", "sre-lead"],
            "location": "meeting-room-4 / dial-in-8831",
            "start": "2026-10-05T09:00:00Z", "end": "2026-10-05T10:00:00Z",
            "decision": "the eu-west-1 node addition ships before the vendor certification window opens",
            "action_items": ["draft the node order by friday afternoon", "confirm the vendor quote with procurement",
                             "book a migration window with the payments team", "update the capacity runbook"],
            "risk": "the vendor lead time slipped to six weeks and the compliance soak needs a full week",
            "owner": "infra-platform", "status": "open"}),
        ),
    ];
    answers
        .into_iter()
        .map(|(label, answer)| (label, extracted(&answer)))
        .collect()
}

/// The hub-distilled distribution, at the default eight neighbours and over the
/// whole bank.
///
/// The stop this exists for: a median above the cap would mean the common case
/// falls back to a reference and the channel ships inert.
#[test]
fn the_hub_distilled_median_fits_the_cap_at_the_default_neighbour_count() {
    let bank = hub_bank();
    let at_default: Vec<(&'static str, Value)> = bank
        .iter()
        .filter(|(_, s)| s["neighbor_count"] == json!(DEFAULT_HUB_NEIGHBORS))
        .map(|(label, s)| (*label, s.clone()))
        .collect();
    assert!(
        at_default.len() >= bank.len() - 2,
        "most of the hub bank is at the default neighbour count: {} of {}",
        at_default.len(),
        bank.len()
    );
    let at_default = measure("hub-distilled at the default neighbour count", &at_default);
    let whole = measure("hub-distilled, whole bank", &bank);
    assert!(
        at_default.median <= EXACT_RENDER_MAX_BYTES,
        "the hub-distilled median is {} bytes at the default {} neighbours, over the {} byte cap: the common case would fall back to a reference",
        at_default.median,
        DEFAULT_HUB_NEIGHBORS,
        EXACT_RENDER_MAX_BYTES
    );
    assert!(whole.rendered > 0, "the hub bank has renderings: {whole:?}");
}

/// The template-extracted distribution over the bank.
///
/// The producer is unbounded, so the range matters as much as the median: the
/// common case must still fit the cap, while the bank must also hold outputs on
/// both sides of it. A range narrowed until every fixture fits would be the
/// fixture's realism lowered to make a number pass.
#[test]
fn the_template_extracted_distribution_is_reported() {
    let bank = template_bank();
    let dist = measure("template-extracted", &bank);
    assert!(dist.min < dist.max, "the bank spans a range: {dist:?}");
    assert_eq!(
        dist.rendered, dist.within_cap,
        "a rendering is published exactly when the snapshot fits: {dist:?}"
    );
    assert!(
        dist.rendered < dist.count,
        "an unbounded producer has snapshots the cap cannot publish, or the range was narrowed: {dist:?}"
    );
    assert!(
        dist.median <= EXACT_RENDER_MAX_BYTES,
        "the template-extracted median is {} bytes, over the {} byte cap: the common case would fall back to a reference: {dist:?}",
        dist.median,
        EXACT_RENDER_MAX_BYTES
    );
}

/// The measurement ran on the real projection, and the cap is met by the whole
/// rendering or not at all: never a prefix, never a shortening.
#[test]
fn a_rendering_is_the_whole_projection_or_nothing_at_all() {
    let snapshot = hub(BANK, MCP, 0, N);
    assert_eq!(
        snapshot["unit_id"],
        json!("hub-distill:default:itm-mcp-0"),
        "the projection carries the record's own unit id and its claim-set count"
    );
    assert_eq!(
        render_exact(&snapshot),
        serde_json::to_string(&snapshot).ok(),
        "a rendering is the whole snapshot, never a prefix"
    );
    let oversized = json!({"amount": "x".repeat(EXACT_RENDER_MAX_BYTES + 1)});
    assert!(
        render_exact(&oversized).is_none(),
        "an over-cap snapshot has no rendering"
    );
}
