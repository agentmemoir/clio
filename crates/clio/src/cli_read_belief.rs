// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` triple and belief read commands:
//! `triple query` and `belief history`.
//!
//! ## Owns
//! - Flag/positional decoding for the bi-temporal triple query and the belief
//!   confidence trajectory, plus their human text views
//!
//! ## Does not own
//! - Triple/belief semantics (see `clio-store` / `clio-belief`)
//! - The read engine and in-process call bridge (see `cli_read`)
//!
//! ## Boundary
//!
//! Changes to a triple/belief read command's flags or views should normally
//! require changes here. Changes to belief storage should not require changes here.
//!
//! Keep this module focused on the triple and belief read commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    add_requested_limit, add_truncated, one_positional, page_limit, parse_u64, read_args,
    validate_time_axis,
};
use crate::cli_read_group::ReadGroup;
use serde_json::Value;

/// The triple and belief read commands.
pub(crate) struct BeliefGroup;

impl ReadGroup for BeliefGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["triple query", "belief history"]
    }

    // The registry record owns each verb's accepted set, so the parser and the
    // usage line cannot disagree; a name with no record accepts only the globals.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(record) => record.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
        match verb {
            "triple query" => triple_query_call(parsed),
            _ => belief_history_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "triple query" => render_triples(payload),
            _ => render_belief(payload),
        }
    }

    fn decorate(&self, verb: &str, parsed: &ParsedArgs, _arguments: &Value, payload: &mut Value) {
        if verb == "triple query" {
            // The JSON envelope carries `total`/`offset`, so the marker reads
            // the same way as `inspect`'s.
            add_truncated(payload, "triples");
            // The CLI clamped a typed `--limit` above the cap; name the value
            // the caller asked for so the served page never silently differs.
            add_requested_limit(payload, parsed);
        }
    }
}

/// Triple pattern terms, in the positional order `SUBJECT PREDICATE OBJECT`.
const TRIPLE_TERMS: &[&str] = &["subject", "predicate", "object"];

fn triple_query_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let positionals = parsed.positionals();
    if positionals.len() > TRIPLE_TERMS.len() {
        return Err(crate::cli_read_args::unexpected_extra(
            parsed,
            TRIPLE_TERMS.len(),
            Some("triple query"),
            "[SUBJECT [PREDICATE [OBJECT]]]",
        ));
    }
    let mut args = read_args(parsed);
    for (term, value) in TRIPLE_TERMS.iter().zip(positionals) {
        args.insert((*term).into(), Value::from(value.clone()));
    }
    for term in TRIPLE_TERMS {
        if let Some(value) = parsed.value(term) {
            args.insert((*term).into(), Value::from(value));
        }
    }
    if let Some(as_of) = parsed.value("as-of") {
        args.insert("as_of".into(), Value::from(as_of));
    }
    if let Some(axis) = parsed.value("time-axis") {
        args.insert("time_axis".into(), Value::from(validate_time_axis(axis)?));
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
    Ok(("triple_query", Value::Object(args)))
}

fn belief_history_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let target = one_positional(parsed, "belief history", "BELIEF_ID_OR_PROPOSITION")?;
    let mut args = read_args(parsed);
    args.insert("belief_id_or_proposition".into(), Value::from(target));
    Ok(("belief_history", Value::Object(args)))
}

fn render_triples(payload: &Value) -> String {
    let triples = payload.get("triples").and_then(Value::as_array);
    let returned = triples.map_or(0, Vec::len);
    let total = payload.get("total").and_then(Value::as_u64);
    let total = total
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(returned);
    let mut header = format!("triples: {returned} of {total}");
    // The page the caller asked for was lowered to the cap; name both values
    // so the header never silently disagrees with the request.
    if let (Some(limit), Some(requested)) = (
        payload.get("limit").and_then(Value::as_u64),
        payload.get("requested_limit").and_then(Value::as_u64),
    ) {
        use std::fmt::Write as _;
        let _ = write!(header, " (limit {limit}, lowered from {requested})");
    }
    let mut lines = vec![header];
    if let Some(triples) = triples {
        for triple in triples {
            lines.push(format!(
                "{} {} {}",
                triple.get("subject").and_then(Value::as_str).unwrap_or("?"),
                triple
                    .get("predicate")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                triple.get("object").and_then(Value::as_str).unwrap_or("?")
            ));
        }
    }
    lines.join("\n")
}

fn render_belief(payload: &Value) -> String {
    let mut lines = vec![format!(
        "belief_id: {}",
        payload
            .get("belief_id")
            .and_then(Value::as_str)
            .unwrap_or("?")
    )];
    if let Some(proposition) = payload.get("proposition").and_then(Value::as_str) {
        lines.push(format!("proposition: {proposition}"));
    }
    let entries = payload
        .get("entries")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    lines.push(format!("entries: {entries}"));
    if let Some(confidence) = payload.get("current_confidence").and_then(Value::as_f64) {
        lines.push(format!("current_confidence: {confidence}"));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "cli_read_belief_tests.rs"]
mod tests;
