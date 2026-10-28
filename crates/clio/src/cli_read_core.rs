// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the core `clio` read commands bound by the
//! shared read contract: `recall`, `get`/`show`, `inspect`, and `stats`.
//!
//! ## Owns
//! - Flag decoding for the core read verbs into MCP tool arguments
//! - The `recall` score-floor, entity-inclusion, and temporal-anchor flag mapping
//! - The `recall` echoed effective limit, its `--limit` clamp disclosure, and
//!   the score-floor warning rewrite
//! - The `inspect` truncation decoration, the `--limit` clamp disclosure, and
//!   their human text views
//!
//! ## Does not own
//! - The read engine, parsing, and in-process call bridge (see `cli_read`)
//! - The MCP tool semantics or gating (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to a core read command's flags or arguments should normally require
//! changes here. Changes to the read engine should not require changes here.
//!
//! Keep this module focused on the core read commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    add_requested_limit, add_truncated, base_args, no_positionals, one_positional, page_limit,
    parse_f64, parse_u64, validate_choice, validate_time_axis,
};
use crate::cli_read_group::ReadGroup;
use serde_json::{Map, Value};

/// The core read commands (`recall`, `get`, `show`, `inspect`, `stats`).
pub(crate) struct CoreGroup;

impl ReadGroup for CoreGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["recall", "get", "show", "inspect", "stats"]
    }

    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(spec) => spec.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
        match verb {
            "recall" => recall_call(parsed),
            "get" | "show" => get_call(verb, parsed),
            "inspect" => inspect_call(parsed),
            _ => stats_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, palette: crate::cli_style::Palette) -> String {
        crate::cli_read_render::render_text(verb, payload, palette)
    }

    fn decorate(&self, verb: &str, parsed: &ParsedArgs, arguments: &Value, payload: &mut Value) {
        if verb == "inspect" {
            add_truncated(payload);
            // The CLI clamped a typed `--limit` above the cap; name the value
            // the caller asked for so the served page never silently differs.
            add_requested_limit(payload, parsed);
        }
        if verb == "recall" {
            // Echo the query and the limit that actually ran, so the text view
            // can head the results and the caller can see a defaulted limit.
            if let Some(map) = payload.as_object_mut() {
                if let Ok(query) = one_positional(parsed, "recall", "QUERY") {
                    map.insert("query".into(), Value::from(query));
                }
                // The value `build` sent, defaulted when the caller did not type
                // `--limit`, so the echo cannot drift from the request.
                let limit = arguments
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(clio_retrieve::DEFAULT_LIMIT as u64);
                map.insert("limit".into(), Value::from(limit));
            }
            // The CLI clamped a typed `--limit` above the cap; name the value
            // the caller asked for so the served page never silently differs.
            add_requested_limit(payload, parsed);
            name_floor_warnings(payload);
        }
    }
}

fn recall_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let query = one_positional(parsed, "recall", "QUERY")?;
    let mut args = base_args(parsed);
    args.insert("query".into(), Value::from(query));
    if let Some(limit) = parsed.value("limit") {
        // One shared page-size rule: `0` refused, above the cap clamped (the
        // engine's own zero-limit refusal stays as the backstop for direct
        // callers). Omitted keeps the engine's default of 10.
        let served = page_limit("limit", Some(parse_u64("limit", limit)?))?;
        args.insert("limit".into(), Value::from(served));
    }
    if let Some(domains) = parsed.value("domains") {
        args.insert("domains".into(), Value::from(split_domains(domains)));
    }
    if let Some(as_of) = parsed.value("as-of") {
        args.insert("as_of".into(), Value::from(as_of));
    }
    if let Some(anchor) = parsed.value("query-anchor") {
        args.insert("query_anchor".into(), Value::from(anchor));
    }
    if let Some(axis) = parsed.value("time-axis") {
        args.insert("time_axis".into(), Value::from(validate_time_axis(axis)?));
    }
    if let Some(scores) = min_scores_arg(parsed)? {
        args.insert("min_scores".into(), scores);
    }
    if parsed.has("no-entities") {
        args.insert("include_entities".into(), Value::Bool(false));
    }
    if parsed.has("entity-summary") {
        args.insert("entity_summary".into(), Value::Bool(true));
    }
    Ok(("retrieve", Value::Object(args)))
}

/// Rewrite the tool's floor names in a `recall` payload's warnings into the
/// flags the caller typed.
///
/// The retrieval layer names a binding floor by its wire name
/// (`min_scores.final`) so any caller can read it; a shell caller typed
/// `--min-final`. The same vocabulary that translates a tool error translates
/// the warning, so the two can never disagree. Every other warning is passed
/// through untouched.
fn name_floor_warnings(payload: &mut Value) {
    let Some(warnings) = payload.get_mut("warnings").and_then(Value::as_array_mut) else {
        return;
    };
    for warning in warnings {
        if let Some(text) = warning.as_str() {
            let named = crate::cli_error::name_cli_inputs(text);
            *warning = Value::from(named);
        }
    }
}

/// The `min_scores` object built from the floor flags the caller actually gave.
///
/// A flag that was not given contributes no key at all, so an unsupplied floor
/// stays unset and imposes no floor; an all-unsupplied call sends no object
/// either, which is why this returns `None` for the default call. Every floor is
/// inclusive, and `semantic`/`keyword` prune only the arm they name before
/// fusion while `reranker`/`final` filter the ranked page.
///
/// A floor is a caller filter over a relative score, not a calibrated relevance
/// threshold, so the per-scale range check belongs to the tool: this layer
/// rejects only what it cannot parse or a value no floor could ever be.
///
/// # Errors
/// A value that is not a finite number, or a negative floor.
fn min_scores_arg(parsed: &ParsedArgs) -> Result<Option<Value>, CliError> {
    let mut scores = Map::new();
    for (flag, key) in [
        ("min-semantic", "semantic"),
        ("min-keyword", "keyword"),
        ("min-reranker", "reranker"),
        ("min-final", "final"),
    ] {
        let Some(raw) = parsed.value(flag) else {
            continue;
        };
        let floor = parse_f64(flag, raw)?;
        if floor < 0.0 {
            return Err(CliError::usage_hint(
                format!("--{flag} must be >= 0, got `{raw}`"),
                format!("try `--{flag} 0`"),
            ));
        }
        scores.insert(key.into(), Value::from(floor));
    }
    Ok((!scores.is_empty()).then_some(Value::Object(scores)))
}

fn get_call(verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let item_id = one_positional(parsed, verb, "ID")?;
    let (snapshot, gist) = (parsed.has("snapshot"), parsed.has("gist"));
    if snapshot && gist {
        return Err(CliError::usage_hint(
            "--snapshot and --gist are mutually exclusive",
            "choose exactly one of `--snapshot` or `--gist`",
        ));
    }
    let tool = if snapshot {
        "get_snapshot"
    } else if gist {
        "get_gist"
    } else {
        "get"
    };
    let mut args = base_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    Ok((tool, Value::Object(args)))
}

fn inspect_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    let mut filter = match parsed.value("filter") {
        Some(raw) => parse_filter(raw)?,
        None => Map::new(),
    };
    for (flag, key) in [
        ("category", "category"),
        ("epistemic-kind", "epistemic_kind"),
        ("time-start", "time_start"),
        ("time-end", "time_end"),
    ] {
        if let Some(value) = parsed.value(flag) {
            filter.insert(key.into(), Value::from(value));
        }
    }
    if let Some(validity) = parsed.value("validity") {
        filter.insert(
            "validity".into(),
            Value::from(validate_choice(
                "validity",
                validity,
                &["all", "active", "invalidated", "discarded"],
            )?),
        );
    }
    if parsed.has("include-discarded") {
        filter.insert("include_discarded".into(), Value::Bool(true));
    }
    let mut args = base_args(parsed);
    if !filter.is_empty() {
        args.insert("filter".into(), Value::Object(filter));
    }
    // One shared page-size rule: default when omitted, `0` refused, above the
    // cap clamped (the clamp is disclosed in `decorate`).
    let requested = parsed
        .value("limit")
        .map(|raw| parse_u64("limit", raw))
        .transpose()?;
    args.insert("limit".into(), Value::from(page_limit("limit", requested)?));
    if let Some(offset) = parsed.value("offset") {
        args.insert("offset".into(), Value::from(parse_u64("offset", offset)?));
    }
    Ok(("inspect", Value::Object(args)))
}

fn stats_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    Ok(("stats", Value::Object(base_args(parsed))))
}

/// Parse `--filter` into the inspect tool's filter object.
///
/// The two failures are reported in the CLI's own words rather than by quoting a
/// JSON parser, because "key must be a string at line 1 column 2" describes the
/// parser's position in its own input, not what the caller has to change. The
/// accepted keys are named, because none of them is discoverable from the
/// command line otherwise.
///
/// # Errors
/// A usage error (exit 2) naming the accepted keys, when `raw` is not a JSON
/// object whose keys are all accepted.
fn parse_filter(raw: &str) -> Result<Map<String, Value>, CliError> {
    let accepted = "accepted keys: category, epistemic_kind, validity, time_start, time_end, include_discarded";
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| CliError::usage_hint("--filter must be a JSON object", accepted))?;
    let Value::Object(map) = value else {
        return Err(CliError::usage_hint(
            "--filter must be a JSON object",
            accepted,
        ));
    };
    let unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !FILTER_KEYS.contains(key))
        .collect();
    if let Some(first) = unknown.first() {
        return Err(CliError::usage_hint(
            format!("--filter key `{first}` is not accepted"),
            accepted,
        ));
    }
    Ok(map)
}

/// The keys `inspect --filter` reads, matching the inspect tool's filter object.
const FILTER_KEYS: &[&str] = &[
    "category",
    "epistemic_kind",
    "validity",
    "time_start",
    "time_end",
    "include_discarded",
];

fn split_domains(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
        .map(str::to_owned)
        .collect()
}

// The temporal-anchor flag tests live in their own child module so both files
// keep real headroom under the 450-line source-file limit.
#[cfg(test)]
#[path = "cli_read_core_temporal_tests.rs"]
mod cli_read_core_temporal_tests;

#[cfg(test)]
#[path = "cli_read_core_recall_skip_tests.rs"]
mod cli_read_core_recall_skip_tests;
