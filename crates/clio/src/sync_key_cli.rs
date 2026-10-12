// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio sync` key-management verbs:
//! `keygen`, `key export`, and `key status`, with their dispatch, help, and
//! refusals.
//!
//! ## Owns
//! - `keygen`, `key export`, and `key status` with their own dispatch
//! - Recording which store a generated key was written to
//! - The argument refusals the three verbs share
//!
//! ## Does not own
//! - Generating, storing, reading, and resolving the key a push or pull sends;
//!   the sync tool reads the recorded store and `CLIO_SYNC_KEY_FILE` itself
//!   (see `clio-config`'s `sync_key`)
//! - `sync status`, `push`, `pull`, and `serve` (see `sync_cli`)
//! - The verbs' static records (see `cli_command_registry_sync`) and the help
//!   surfaces that render them (see `cli_help`, `cli_help_usage`, and
//!   `command_catalog`)
//!
//! ## Boundary
//!
//! Changes to a key verb's flags, usage text, or summary should normally
//! require changes in `cli_command_registry_sync`; changes to its dispatch,
//! output, or refusals should normally require changes here. Changes to how a
//! transfer key is resolved or to how a key is stored should not require
//! changes here.
//!
//! Keep this module focused on the key verbs.
//!
//! # Why this is not part of `sync_cli`
//!
//! The key verbs dispatch differently: `keygen` sits at the top level while
//! `export` and `status` nest under `key`, and the shared module stays focused
//! on the four transfer verbs. `sync_cli` was also within two lines of the
//! workspace file-size limit, so the views moved out of it rather than into it.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_config::KeySource;
use clio_config::sync_key::{self, SyncKeyEntry, SyncKeyLocation, SyncKeyReport};
use serde_json::json;
use std::process::ExitCode;

/// The note keygen prints: the one command that can show the key again.
const RECOVERY_NOTE: &str = "the key cannot be recovered except with `clio sync key export`";

/// The second note a rotation prints, only when a previous key was kept.
const ROTATION_NOTE: &str = "the previous key is retained, so older content stays readable; peers need this new key before they can read newly sealed content";

/// The note export carries: printing the key is deliberate, and avoidable.
const EXPORT_NOTE: &str = "exporting prints the key on purpose; a shell flag or environment variable leaves the key in process listings, so a shared deployment should pass CLIO_SYNC_KEY_FILE pointing at an owner-only key file";

/// The key commands the unknown-subcommand suggestion draws on.
///
/// `keygen` is in this list because `clio sync key keygen` is a mistake an
/// operator can make, and the hint it gets is the command that does exist.
const SUBCOMMANDS: &[&str] = &["keygen", "export", "status"];

/// Dispatch `clio sync keygen` and `clio sync key <subcommand>`.
pub(crate) fn sync_key(args: Vec<String>) -> ExitCode {
    let env = |key: &str| std::env::var(key).ok();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut input = std::io::stdin();
    let mut io = CliIo {
        out: &mut out,
        err: &mut err,
        stdin: &mut input,
        is_tty: cli_output::stdout_is_tty(),
        no_color: cli_output::no_color_requested(),
        env: &env,
    };
    run(&mut io, args)
}

/// Resolve, parse, and run one key verb.
///
/// The injection seam for tests: the streams, terminal state, and environment
/// come from `io`, so no test reaches the process environment or the machine's
/// real key store.
pub(crate) fn run(io: &mut CliIo, args: Vec<String>) -> ExitCode {
    let fallback = cli_output::mode_from_raw(&args, io.is_tty);
    let mut tokens = args.into_iter();
    let Some(first) = tokens.next() else {
        return fail(io, fallback, &missing_subcommand());
    };
    let verb = match first.as_str() {
        "keygen" => "sync keygen",
        "key" => match tokens.next().as_deref() {
            Some("--help" | "-h") => {
                let _ = cli_output::write_text(&mut io.out, &key_help());
                return ExitCode::from(EXIT_OK);
            }
            Some("export") => "sync key export",
            Some("status") => "sync key status",
            Some(sub) if !sub.starts_with('-') => {
                return fail(io, fallback, &unknown_subcommand(sub));
            }
            _ => return fail(io, fallback, &missing_subcommand()),
        },
        other => return fail(io, fallback, &unknown_subcommand(other)),
    };
    let parsed = match cli_args::parse(&spec(verb), tokens) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    if parsed.has("help") {
        return match crate::cli_help_usage::help_for(verb) {
            Some(usage) => {
                let _ = cli_output::write_text(&mut io.out, &usage);
                ExitCode::from(EXIT_OK)
            }
            None => fail(
                io,
                fallback,
                &CliError::usage(format!("no help is registered for `{verb}`")),
            ),
        };
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    match verb {
        "sync keygen" => keygen(io, &parsed, mode),
        "sync key export" => export(io, &parsed, mode),
        _ => status(io, &parsed, mode),
    }
}

/// The flags one key verb accepts, globals included.
///
/// Decoded through the registry, plus the globals every parser spec merges:
/// neither `sync_cli`'s verbs nor their flags can reach these records.
fn spec(verb: &str) -> FlagSpec {
    match crate::cli_command_registry::find(verb) {
        Some(spec) => spec.parser_spec(),
        None => FlagSpec::new(&[], &[]),
    }
}

/// Refuse the store flags a key verb cannot honor.
///
/// The key verbs read one file or one keystore entry and open no database, so
/// `--db` and `--backend` would change nothing while looking as though they had.
/// Saying so beats accepting a flag that cannot change the answer, which is the
/// same rule the backup verb family follows.
fn refuse_store_flags(verb: &str, parsed: &ParsedArgs) -> Result<(), CliError> {
    for flag in ["db", "backend"] {
        if parsed.value(flag).is_some() {
            return Err(CliError::usage(format!(
                "`clio {verb}` reads the sync key, not a store; `--{flag}` cannot change the key"
            ))
            .with_hint("drop the flag, or set CLIO_SYNC_KEY_FILE to point at the key file"));
        }
    }
    Ok(())
}

/// The two verbs nested under `clio sync key`.
///
/// `clio sync keygen` is deliberately absent: it is a `clio sync` subcommand, and
/// listing it here is what made this help claim a command that is not one of
/// these.
const NESTED_SUBCOMMANDS: &[&str] = &["export", "status"];

/// `sync key --help`: the nested key commands, grouped, plus `sync keygen`.
///
/// Rendered by the shared group renderer so the binding marker beside each verb is
/// the same one `clio sync --help` prints, rather than a second hand-written list
/// that can only ever agree with it by accident. `sync keygen` is named on its
/// own line because `clio sync --help` lists it under `clio sync`.
fn key_help() -> String {
    let mut help = crate::cli_help::group_help("sync key", NESTED_SUBCOMMANDS);
    help.push_str(
        "\n  clio sync keygen   create or supersede the key; printed once; the previous key is kept in the archive beside the key file, so older content stays readable (a `clio sync` subcommand)",
    );
    help
}

/// The two argument refusals `keygen` shares with its siblings.
///
/// Grouped so all three verbs cannot drift apart on what they accept.
fn keygen_refusals(parsed: &ParsedArgs) -> Result<(), CliError> {
    crate::runtime_cli::no_positionals(parsed)?;
    refuse_store_flags("sync keygen", parsed)
}

/// The same two refusals, named for the two read-only verbs.
fn transfer_refusals(verb: &str, parsed: &ParsedArgs) -> Result<(), CliError> {
    crate::runtime_cli::no_positionals(parsed)?;
    refuse_store_flags(verb, parsed)
}

/// The error for no subcommand at all, pointing at the read-only default.
fn missing_subcommand() -> CliError {
    CliError::usage("`sync key` requires a subcommand").with_hint("try `clio sync key status`")
}

/// The error for an unknown key subcommand, suggesting the closest real one.
fn unknown_subcommand(sub: &str) -> CliError {
    let hint = cli_args::suggest(sub, SUBCOMMANDS).map_or_else(
        || "try `clio sync key status`".to_owned(),
        |best| {
            if best == "keygen" {
                "did you mean `clio sync keygen`?".to_owned()
            } else {
                format!("did you mean `clio sync key {best}`?")
            }
        },
    );
    CliError::usage(format!("unknown `sync key` subcommand `{sub}`")).with_hint(hint)
}

/// `sync keygen [--store file|keystore]`.
///
/// The generated key is printed exactly once, and is never written to stderr or
/// to an error message. `generate` runs in a fixed order: refuse the
/// shadowing-custody shape, record the store choice, archive the current key
/// and delete it, then write the replacement. A refusal before the record
/// leaves the machine as it found it. A failure after the archive leaves the
/// node with archived keys and no current key, which is not as it found it and
/// is repaired by running `keygen` again.
fn keygen(io: &mut CliIo, parsed: &ParsedArgs, mode: OutputMode) -> ExitCode {
    if let Err(error) = keygen_refusals(parsed) {
        return fail(io, mode, &error);
    }
    let env_fn = |key: &str| (io.env)(key);
    let requested = match parsed.value("store") {
        Some(raw) => match KeySource::parse(raw) {
            Some(source) => Some(source),
            None => {
                return fail(
                    io,
                    mode,
                    &CliError::usage_hint(
                        format!("--store must be one of {}", KeySource::accepted()),
                        format!("try `--store {}`", KeySource::recommended().as_str()),
                    ),
                );
            }
        },
        None => None,
    };
    // `generate` records the store choice itself, before it writes a key byte, so
    // a marker that cannot be written fails here with nothing left behind and the
    // message below never has to describe a key it is withholding.
    let loaded = match sync_key::generate(&env_fn, requested) {
        Ok(loaded) => loaded,
        Err(error) => return fail(io, mode, &CliError::from_am_error(&error)),
    };
    let source = store_of(&loaded.location);
    let key = loaded.base64();
    // The report reads back what the rotation left behind; more than one key
    // means a key was kept. When the report cannot be read the count stays
    // unknown, because a guessed 1 would tell a node holding several keys it
    // holds one.
    let report = sync_key::report(&env_fn).ok();
    let rotated = report.as_ref().is_some_and(|rep| rep.count > 1);
    let mut text = format!("{key}\n{RECOVERY_NOTE}");
    let mut payload = json!({
        "ok": true,
        "store": source.as_str(),
        "fingerprint": loaded.fingerprint(),
        "created_at": report.as_ref().and_then(|rep| rep.created_at.clone()),
        "count": report.as_ref().map(|report| report.count),
        "key": key,
        "note": RECOVERY_NOTE,
    });
    if rotated {
        text.push('\n');
        text.push_str(ROTATION_NOTE);
        payload["rotation_note"] = json!(ROTATION_NOTE);
    }
    emit(io, mode, &payload, &text);
    ExitCode::from(EXIT_OK)
}

/// `sync key export`.
///
/// This is the only other command that prints the key. Missing means the store
/// was read and holds nothing, which is an operational failure rather than an
/// empty success.
fn export(io: &mut CliIo, parsed: &ParsedArgs, mode: OutputMode) -> ExitCode {
    if let Err(error) = transfer_refusals("sync key export", parsed) {
        return fail(io, mode, &error);
    }
    let env_fn = |key: &str| (io.env)(key);
    let loaded = match sync_key::load(&env_fn) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => {
            return fail(
                io,
                mode,
                &CliError::operational(
                    "not_found",
                    "no sync key is stored; run `clio sync keygen`",
                ),
            );
        }
        Err(error) => return fail(io, mode, &CliError::from_am_error(&error)),
    };
    let key = loaded.base64();
    let text = key.clone();
    let payload = json!({
        "ok": true,
        "store": store_of(&loaded.location).as_str(),
        "fingerprint": loaded.fingerprint(),
        "key": key,
        "note": EXPORT_NOTE,
    });
    emit(io, mode, &payload, &text);
    ExitCode::from(EXIT_OK)
}

/// `sync key status`.
///
/// Reports the store, fingerprint, creation time, and key count. The key and
/// every prefix of it stay out of both output modes.
fn status(io: &mut CliIo, parsed: &ParsedArgs, mode: OutputMode) -> ExitCode {
    if let Err(error) = transfer_refusals("sync key status", parsed) {
        return fail(io, mode, &error);
    }
    let env_fn = |key: &str| (io.env)(key);
    let report = match sync_key::report(&env_fn) {
        Ok(report) => report,
        Err(error) => return fail(io, mode, &CliError::from_am_error(&error)),
    };
    let text = status_text(&report);
    let payload = json!({
        "ok": true,
        "store": report.store.as_str(),
        "fingerprint": report.fingerprint,
        "created_at": report.created_at,
        "count": report.count,
        "keys": keys_payload(&report.keys),
    });
    emit(io, mode, &payload, &text);
    ExitCode::from(EXIT_OK)
}

/// The per-key facts the status outputs show; never a key byte or a prefix.
fn keys_payload(keys: &[SyncKeyEntry]) -> Vec<serde_json::Value> {
    keys.iter()
        .map(|entry| {
            json!({
                "fingerprint": entry.fingerprint, "created_at": entry.created_at,
                "current": entry.current, "store": entry.store,
            })
        })
        .collect()
}

/// The human status view: one header, then one line per held key, or the
/// no-key line when the store holds nothing.
fn status_text(report: &SyncKeyReport) -> String {
    let mut lines = vec![format!(
        "sync key status (store {}, keys {})",
        report.store, report.count
    )];
    if report.keys.is_empty() {
        lines.push("no sync key is stored; run `clio sync keygen`".to_owned());
        return lines.join("\n");
    }
    for entry in &report.keys {
        let created = entry.created_at.as_deref().map_or_else(
            || "created at unavailable".to_owned(),
            |created| format!("created at {created}"),
        );
        let current = if entry.current { " (current)" } else { "" };
        lines.push(format!(
            "fingerprint {} store {} {created}{current}",
            entry.fingerprint, entry.store
        ));
    }
    lines.join("\n")
}

/// Which store a loaded key came from, as the recorded choice.
fn store_of(location: &SyncKeyLocation) -> KeySource {
    match location {
        SyncKeyLocation::File(_) => KeySource::File,
        SyncKeyLocation::Keystore => KeySource::Keystore,
    }
}

#[cfg(test)]
#[path = "sync_key_cli_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_key_cli_rotation_tests.rs"]
mod rotation_tests;

#[cfg(test)]
#[path = "sync_key_cli_rejection_tests.rs"]
mod rejection_tests;
