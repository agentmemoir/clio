// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio batch` command: reading ordered
//! write operations from a JSON file (or stdin via `--file -`) and handing them
//! to the atomic `batch` tool, with `--dry-run` reporting the plan without any
//! write.
//!
//! ## Owns
//! - The operations file/stdin decoding (fail-closed before any dispatcher call)
//! - The `batch` flag decoding and human text view
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - Batch pre-flight, `$ref` resolution, or atomicity (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to the operations-file format or batch flags should normally require
//! changes here. Changes to batch atomicity or gating should not require
//! changes here.
//!
//! Keep this module focused on the `batch` command.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{CliIo, base_args};
use crate::cli_write_group::{Refusal, WriteCall, WriteGroup};
use serde_json::Value;

/// The atomic batch write command.
pub(crate) struct BatchWriteGroup;

impl WriteGroup for BatchWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["batch"]
    }

    // The registry record owns each verb's accepted set, so the parser and the
    // usage line cannot disagree; a name with no record accepts only the globals.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(record) => record.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, _verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        // No injected streams on this path: `--file -` cannot read stdin here.
        batch_call(parsed, None)
    }

    fn build_with_io(
        &self,
        _verb: &str,
        parsed: &ParsedArgs,
        io: &mut CliIo<'_>,
    ) -> Result<WriteCall, CliError> {
        batch_call(parsed, Some(&mut *io.stdin))
    }

    fn render(&self, _verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        let count = payload
            .get("operations")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if payload.get("dry_run") == Some(&Value::Bool(true)) {
            format!("batch dry-run: {count} operations would commit (zero writes)")
        } else {
            format!("batch: {count} operations committed")
        }
    }

    fn mutates(&self, _verb: &str) -> bool {
        true
    }

    fn refusal(&self, _verb: &str, _payload: &Value) -> Option<Refusal> {
        None
    }
}

/// `batch --file ops.json [--dry-run]`: decode the operations file (or stdin
/// via `--file -`) fail-closed, then hand the array to the atomic `batch` tool.
/// `bank`/`actor` go to the batch call, never into individual operations.
fn batch_call(
    parsed: &ParsedArgs,
    stdin: Option<&mut dyn std::io::Read>,
) -> Result<WriteCall, CliError> {
    let path = parsed.value("file").ok_or_else(|| {
        CliError::usage_hint(
            "`batch` requires --file (an operations JSON file, or `-` for stdin)",
            "try 'clio batch --file ops.json'",
        )
    })?;
    let raw = match path {
        "-" => {
            let Some(stdin) = stdin else {
                return Err(CliError::usage_hint(
                    "`batch --file -` reads the operations JSON from stdin",
                    "try 'clio batch --file - < ops.json'",
                ));
            };
            std::io::read_to_string(stdin).map_err(|error| {
                CliError::usage(format!("`batch --file -` is not readable: {error}"))
            })?
        }
        other => std::fs::read_to_string(other).map_err(|error| {
            CliError::usage(format!("--file `{other}` is not readable: {error}"))
        })?,
    };
    let operations = parse_operations(&raw)?;
    let mut args = base_args(parsed);
    args.insert("operations".into(), Value::Array(operations));
    if parsed.has("dry-run") {
        args.insert("dry_run".into(), Value::Bool(true));
    }
    Ok(WriteCall::Tool {
        tool: "batch",
        args: Value::Object(args),
    })
}

/// `admit --file ops.json`: the file's `items` go to the read-only
/// `admit_preview_batch` tool. A command-level TEXT or `--context` has no item
/// to land on, so either one fails closed instead of being dropped.
pub(crate) fn admit_batch_call(parsed: &ParsedArgs, path: &str) -> Result<WriteCall, CliError> {
    if !parsed.positionals().is_empty() {
        return Err(CliError::usage_hint(
            "`admit --file` does not accept TEXT; put each text in its item object",
            "set `item.gist` on each item in the file",
        ));
    }
    if parsed.value("context").is_some() {
        return Err(CliError::usage_hint(
            "`admit --file` does not accept --context; set `context` on each item",
            "each item object may carry its own bounded `context` string",
        ));
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|error| CliError::usage(format!("--file `{path}` is not readable: {error}")))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| CliError::usage(format!("--file must contain a JSON object: {error}")))?;
    let Some(items) = value.get("items").filter(|items| items.is_array()) else {
        return Err(CliError::usage_hint(
            "--file must contain a JSON object with a non-empty `items` array",
            "try {\"items\": [{\"item\": {\"gist\": \"...\"}, \"category\": \"persona\", \"epistemic_kind\": \"fact\"}]}",
        ));
    };
    let mut args = base_args(parsed);
    args.insert("items".into(), items.clone());
    Ok(WriteCall::Tool {
        tool: "admit_preview_batch",
        args: Value::Object(args),
    })
}

/// Decode the operations document: an object with an `operations` array, or a
/// bare top-level array (both are the same thing to the tool). Anything else
/// fails closed with a usage error before any dispatcher call; an empty array
/// passes through (the tool owns that behavior).
fn parse_operations(raw: &str) -> Result<Vec<Value>, CliError> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        CliError::usage(format!("--file must contain JSON operations: {error}"))
    })?;
    match value {
        Value::Array(operations) => Ok(operations),
        Value::Object(fields) => match fields.get("operations") {
            Some(Value::Array(operations)) => Ok(operations.clone()),
            Some(_) => Err(CliError::usage(
                "--file `operations` must be a JSON array of `{tool, params}` operations",
            )),
            None => Err(CliError::usage_hint(
                "--file must contain a JSON object with an `operations` array",
                "try {\"operations\": [{\"tool\": \"store\", \"params\": {\"item\": {\"id\": \"...\", \"gist\": \"...\"}, \"category\": \"persona\", \"epistemic_kind\": \"fact\"}}]}",
            )),
        },
        _ => Err(CliError::usage(
            "--file must contain a JSON object with an `operations` array or a bare JSON array",
        )),
    }
}

#[cfg(test)]
#[path = "cli_write_batch_tests.rs"]
mod tests;
