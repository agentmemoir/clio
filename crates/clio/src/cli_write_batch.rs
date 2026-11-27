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
use clio_store::MAX_BATCH_OPS;
use serde_json::Value;

/// What a caller can do about an operations document that is too large for this
/// command. The batch commits as one transaction, so splitting the document
/// across runs would apply it in pieces rather than atomically: the honest
/// alternative is fewer operations in this batch, not two batches.
const TOO_LARGE: &str = "send fewer operations in this batch: a batch commits as one transaction, so splitting it across runs would not apply it atomically";

/// What a caller can do about an admission file that is too large for this
/// command. The file only feeds a preview, so a smaller file is the honest
/// alternative and nothing depends on the items arriving together.
const ADMIT_TOO_LARGE: &str =
    "send a smaller file: `admit --file` reads at most 4 MiB, so remove or shorten items";

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
            crate::cli_read_cap::read_document(stdin, "`batch --file -`", TOO_LARGE)?
        }
        other => {
            let what = format!("--file `{other}`");
            crate::cli_read_cap::read_document_file(other, &what, TOO_LARGE)?
        }
    };
    let operations = parse_operations(&raw)?;
    // The batch tool refuses more than `MAX_BATCH_OPS` operations itself, but
    // only after the document reaches it. Refuse the same count here, so the
    // oversized document is not carried any further. `MAX_BATCH_OPS` is the
    // right bound for this command: it is the atomic executor's own cap, and a
    // second, looser number here would let a document through that the executor
    // must then refuse. It is also far below the shared entry cap, so that cap
    // needs no separate check on this path.
    if operations.len() > MAX_BATCH_OPS {
        return Err(CliError::usage_hint(
            format!(
                "`batch` carries {} operations, over the {MAX_BATCH_OPS}-operation cap",
                operations.len()
            ),
            TOO_LARGE,
        ));
    }
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
    let raw = crate::cli_read_cap::read_document_file(
        path,
        &format!("--file `{path}`"),
        ADMIT_TOO_LARGE,
    )?;
    let mut value: Value = serde_json::from_str(&raw)
        .map_err(|error| CliError::usage(format!("--file must contain a JSON object: {error}")))?;
    // The map is owned here, so the array is taken out of it instead of copied:
    // a second copy of every item is exactly the memory the input cap bounds.
    let Some(items) = value
        .as_object_mut()
        .and_then(|fields| fields.remove("items"))
        .filter(Value::is_array)
    else {
        return Err(CliError::usage_hint(
            "--file must contain a JSON object with a non-empty `items` array",
            "try {\"items\": [{\"item\": {\"gist\": \"...\"}, \"category\": \"persona\", \"epistemic_kind\": \"fact\"}]}",
        ));
    };
    let mut args = base_args(parsed);
    args.insert("items".into(), items);
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
        // The map is owned here, so the array is taken out of it instead of
        // copied: a second copy of every operation is exactly the memory the
        // input cap exists to bound.
        Value::Object(mut fields) => match fields.remove("operations") {
            Some(Value::Array(operations)) => Ok(operations),
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
