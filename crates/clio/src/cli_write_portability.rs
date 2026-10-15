// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` JSON-portability write commands
//! `export`, `import`, and `erase`, decoded into their gated MCP tools with a
//! `--dry-run` no-write preview for erasure, and for the `compliance
//! review-export` binding that runs the in-process `review_export` tool.
//!
//! ## Owns
//! - Flag decoding for these portability verbs into MCP tool arguments (1:1 with
//!   the published `inputSchema`)
//! - The `erase` confirmation gate call and the irreversibility dry-run preview
//! - The `compliance review-export` runner: store open, argument build, and the
//!   in-process `review_export` dispatch
//! - Their human text views
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - The confirmation decision itself (see `cli_confirm`)
//! - Bundle, import, and erasure semantics; the in-process tools own behavior
//!   (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to these commands' flags or arguments should normally require changes
//! here. Changes to the write engine or bundle semantics should not.
//!
//! Keep this module focused on the portability write commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_output::OutputMode;
use crate::cli_read::{
    CliIo, base_args, emit, fail, flag, parse_u64, require_flags, required_flags, validate_choice,
};
use crate::cli_write_group::{Refusal, WriteCall, WriteGroup};
use serde_json::{Map, Value, json};
use std::process::ExitCode;

/// The JSON-portability write commands.
pub(crate) struct PortabilityWriteGroup;

impl WriteGroup for PortabilityWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["export", "import", "erase"]
    }

    // The registry record owns each verb's accepted set, so the parser and the
    // usage line cannot disagree; a name with no record accepts only the globals.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(record) => record.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        match verb {
            "export" => export_call(parsed),
            "import" => import_call(parsed),
            _ => erase_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "export" => render_export(payload),
            "import" => render_import(payload),
            _ => render_erase(payload),
        }
    }

    fn mutates(&self, verb: &str) -> bool {
        // Only `import` enqueues index work: a one-shot process must drain the
        // index so restored items are searchable before the process exits.
        // Export writes outside the store and erasure destroys content.
        verb == "import"
    }

    fn refusal(&self, _verb: &str, _payload: &Value) -> Option<Refusal> {
        // These payloads carry no admission decision.
        None
    }
}

/// `export --out FILE [--content-mode M] [--category C] [--time-start TS]
/// [--time-end TS] [--include-archived B] [--include-discarded B]` → `export`:
/// one bundle file plus its completeness manifest. The tool refuses
/// unprivileged actors (`--actor harness|system`); the CLI passes `--actor`
/// through so the gate stays inside the in-process tool.
fn export_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    require_flags(parsed, "export", required_flags("export"))?;
    let destination = flag(parsed, "out");
    let mut args = base_args(parsed);
    args.insert("destination".into(), Value::from(destination));
    if let Some(mode) = parsed.value("content-mode") {
        let mode = validate_choice(
            "content-mode",
            mode,
            &["dsar_plaintext", "ciphertext_backup"],
        )?;
        args.insert("content_mode".into(), Value::from(mode));
    }
    for (flag, key) in [
        ("category", "category"),
        ("time-start", "time_start"),
        ("time-end", "time_end"),
    ] {
        if let Some(value) = parsed.value(flag) {
            let value = if flag == "category" {
                validate_choice(flag, value, clio_types::SemanticCategory::WIRE)?
            } else {
                value
            };
            args.insert(key.into(), Value::from(value));
        }
    }
    include_flag(parsed, "include-archived", "include_archived", &mut args)?;
    include_flag(parsed, "include-discarded", "include_discarded", &mut args)?;
    if let Some(subject) = parsed.value("subject") {
        args.insert("subject_id".into(), Value::from(subject));
    }
    Ok(WriteCall::Tool {
        tool: "export",
        args: Value::Object(args),
    })
}

/// Decode `--include-archived`/`--include-discarded`: `true`|`false` validated
/// fail-closed and mapped onto the tool's boolean argument (default true).
fn include_flag(
    parsed: &ParsedArgs,
    flag: &str,
    key: &str,
    args: &mut Map<String, Value>,
) -> Result<(), CliError> {
    if let Some(raw) = parsed.value(flag) {
        let value = validate_choice(flag, raw, &["true", "false"])?;
        args.insert(key.into(), Value::Bool(value == "true"));
    }
    Ok(())
}

/// `import --source FILE [--dry-run] [--force --reason R]` → `import`:
/// manifest-validated, idempotent restore into `--bank`. `--force` without
/// `--reason` is rejected inside the tool.
fn import_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    require_flags(parsed, "import", required_flags("import"))?;
    let source = flag(parsed, "source");
    let mut args = base_args(parsed);
    args.insert("source".into(), Value::from(source));
    if parsed.has("dry-run") {
        args.insert("dry_run".into(), Value::Bool(true));
    }
    if parsed.has("force") {
        args.insert("force".into(), Value::Bool(true));
    }
    if let Some(reason) = parsed.value("reason") {
        args.insert("reason".into(), Value::from(reason));
    }
    Ok(WriteCall::Tool {
        tool: "import",
        args: Value::Object(args),
    })
}

/// `erase --subject S --basis B --request R [--reason-code C] --confirm` →
/// `erase_request`: verified compliance crypto-shred. `--dry-run` previews the
/// irreversibility with zero writes; the confirm gate refuses without
/// `--confirm` (exit 2). Erasure is never `discard` and never hygiene cleanup.
fn erase_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    require_flags(parsed, "erase", required_flags("erase"))?;
    let subject = flag(parsed, "subject");
    let basis = flag(parsed, "basis");
    let request = flag(parsed, "request");
    if parsed.has("dry-run") {
        return Ok(WriteCall::Preview {
            payload: json!({
                "dry_run": true,
                "operation": "erase",
                "irreversible": true,
                "subject": subject,
                "would": "destroy the subject's DEK and write content-free tombstones",
            }),
            text: format!(
                "dry-run: would erase subject {subject}\n  \
                 effect: destroy the subject's DEK and write content-free tombstones\n  \
                 warning: compliance erasure is IRREVERSIBLE and is distinct from `discard` \
                 and from hygiene cleanup (hygiene never aliases erasure)\n  \
                 pass --confirm to apply"
            ),
        });
    }
    crate::cli_confirm::require_confirm("erase", parsed)?;
    let mut args = base_args(parsed);
    args.insert("subject_id".into(), Value::from(subject));
    args.insert("legal_basis".into(), Value::from(basis));
    args.insert("request_id".into(), Value::from(request));
    if let Some(reason_code) = parsed.value("reason-code") {
        args.insert("reason_code".into(), Value::from(reason_code));
    }
    args.insert("confirm".into(), Value::Bool(true));
    Ok(WriteCall::Tool {
        tool: "erase_request",
        args: Value::Object(args),
    })
}

fn render_export(payload: &Value) -> String {
    let manifest = payload.get("manifest").unwrap_or(&Value::Null);
    let destination = payload
        .get("destination")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let mode = manifest
        .get("content_mode")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let complete = manifest
        .get("complete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entities = manifest
        .get("counts_by_type")
        .and_then(Value::as_array)
        .map_or(0, |counts| {
            counts
                .iter()
                .filter_map(|entry| entry.get(1).and_then(Value::as_u64))
                .sum()
        });
    let sha = manifest
        .get("content_sha256")
        .and_then(Value::as_str)
        .unwrap_or("");
    format!(
        "export wrote {destination} (content_mode={mode}, complete={complete}, \
         entities={entities}, sha256={}…)",
        sha.get(..12).unwrap_or(sha)
    )
}

fn render_import(payload: &Value) -> String {
    let report = payload.get("report").unwrap_or(&Value::Null);
    let count = |key: &str| report.get(key).and_then(Value::as_u64).unwrap_or(0);
    let (create, skip, reject, overwrite) = (
        count("would_create"),
        count("would_skip"),
        count("would_reject"),
        count("would_overwrite"),
    );
    if report.get("dry_run") == Some(&Value::Bool(true)) {
        return format!(
            "import dry-run: would create {create}, skip {skip}, reject {reject} (zero writes)"
        );
    }
    if report.get("applied") != Some(&Value::Bool(true)) {
        return format!(
            "import applied nothing: would create {create}, skip {skip}, \
             reject {reject}, overwrite {overwrite}"
        );
    }
    format!("import: created {create}, skipped {skip}, rejected {reject}, overwrote {overwrite}")
}

fn render_erase(payload: &Value) -> String {
    let status = payload.get("status").and_then(Value::as_str).unwrap_or("?");
    let request = payload
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let items = payload
        .get("item_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let tombstones = payload
        .get("tombstones_written")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let triples = payload
        .get("triples_purged")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!(
        "erased {items} item(s) for request {request} (status {status}; DEK destroyed; \
         triples {triples}; tombstones {tombstones}; irreversible)"
    )
}

/// Run `clio compliance review-export`: parse the record's flags and export the
/// ordered content-free local review events to `--out` under destination
/// custody, through the in-process `review_export` tool.
///
/// The write itself — destination custody, the per-call action id, and the
/// product staging directory — lives inside the tool, so the CLI and the MCP
/// surface share one writer.
pub(crate) fn run_review_export(
    io: &mut CliIo,
    fallback: OutputMode,
    tokens: Vec<String>,
) -> ExitCode {
    let parsed = match crate::cli_args::parse(&review_export_spec(), tokens)
        .map_err(|error| crate::cli_error::with_help_hint(error, "compliance review-export"))
    {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    let mode = match crate::cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Err(error) = require_flags(
        &parsed,
        "compliance review-export",
        required_flags("compliance review-export"),
    ) {
        return fail(io, mode, &error);
    }
    if let Err(error) = crate::runtime_cli::no_positionals(&parsed) {
        return fail(io, mode, &error);
    }
    let destination = flag(&parsed, "out").to_owned();
    match review_export_write(io, &parsed) {
        Ok(()) => {
            let payload = json!({"ok": true, "destination": destination});
            emit(
                io,
                mode,
                &payload,
                &format!("review export wrote {destination}"),
            );
            ExitCode::from(EXIT_OK)
        }
        Err(error) => fail(io, mode, &error),
    }
}

/// The flags `compliance review-export` accepts: the record's own parser spec,
/// so the accepted flags and the usage text cannot drift.
fn review_export_spec() -> FlagSpec {
    match crate::cli_command_registry::find("compliance review-export") {
        Some(spec) => spec.parser_spec(),
        None => FlagSpec::new(&[], &[]),
    }
}

/// Open the store and run the in-process `review_export` tool against it.
fn review_export_write(io: &CliIo, parsed: &ParsedArgs) -> Result<(), CliError> {
    let get = |key: &str| (io.env)(key);
    let target =
        crate::db_target::resolve_target(&get, parsed.value("db"), parsed.value("backend"))
            .map_err(CliError::usage)?;
    let options = clio_mcp::runtime::McpOpenOptions {
        backend: target.backend,
        database_url: target.url.clone(),
        default_bank: "default".to_owned(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path: clio_config::kms_path::resolve_kms_path(&get, &target.url),
    };
    let state = clio_mcp::runtime::McpState::open_one_shot(&options)
        .map_err(|error| CliError::from_am_error(&error))?;
    let mut args = base_args(parsed);
    args.insert("destination".into(), Value::from(flag(parsed, "out")));
    if let Some(limit) = parsed.value("limit") {
        args.insert("limit".into(), Value::from(parse_u64("limit", limit)?));
    }
    if let Some(cursor) = parsed.value("cursor") {
        args.insert("cursor".into(), Value::from(cursor));
    }
    let payload = clio_mcp::write_tools::dispatch(&state, "review_export", &Value::Object(args));
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let code = payload
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("internal");
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("review export failed");
    Err(CliError::tool_failure(code, message))
}

#[cfg(test)]
#[path = "cli_write_portability_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_write_portability_category_tests.rs"]
mod category_tests;

#[cfg(test)]
#[path = "cli_write_portability_subject_tests.rs"]
mod subject_tests;
