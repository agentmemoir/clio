// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio auth token` CLI surface: adding,
//! listing, and revoking network credentials in the owner-only token store.
//!
//! ## Owns
//! - The `auth token add|list|revoke` verbs and their text views
//! - The single one-time print of a generated secret on `add`
//! - Mapping store failures into the shared CLI error contract
//!
//! ## Does not own
//! - The store itself: records, validation, digests, permissions
//!   (`clio_config::auth_tokens`)
//! - Any transport behavior; no runtime reads the store in this phase
//! - The verbs' static records (see `cli_command_registry_auth_config`)
//! - Output/error contracts (see `cli_read`) or flag parsing (`cli_args`)
//!
//! ## Boundary
//!
//! Changes to an `auth` verb's flags, usage text, or summary should normally
//! require changes in `cli_command_registry_auth_config`; changes to its
//! dispatch, decoding, or views should normally require changes here. Changes
//! to store validation or record shape should not require changes here.
//!
//! Keep this module focused on the `clio auth` surface.
//!
//! # Secret handling
//!
//! `add` is the only verb that ever sees a plaintext secret, and it emits
//! exactly once: the generated value goes to stdout with the store path and a
//! cannot-be-recovered warning. `list` renders labels, actors, banks, and
//! creation times — never a secret and never a digest, in either output mode.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_config::auth_tokens::{
    AuthTokenRecord, add_token, load_auth_tokens, resolve_token_store_path_with, revoke_token,
};
use serde_json::{Value, json};
use std::process::ExitCode;

/// The `auth` subcommands this surface binds: the registered `auth` records'
/// first token past the prefix, once each, in registry order.
///
/// The split is optional: a record with no second token (`auth login`) is its
/// own subcommand rather than being skipped, and a longer name contributes
/// only its first token.
fn auth_subcommands() -> Vec<&'static str> {
    let mut subs: Vec<&'static str> = Vec::new();
    for name in crate::cli_command_registry::names() {
        if let Some(rest) = name.strip_prefix("auth ") {
            let sub = rest.split_once(' ').map_or(rest, |(token, _)| token);
            if !subs.contains(&sub) {
                subs.push(sub);
            }
        }
    }
    subs
}

/// The `auth token` verbs this surface binds: the registered `auth token`
/// records' first token past the prefix, in registry order. None binds an MCP
/// tool: an agent must not be able to mint, widen, or list credentials.
///
/// The split is optional, so a verb-shaped suffix is taken whole and a longer
/// name contributes only its first token.
fn token_verbs() -> Vec<&'static str> {
    crate::cli_command_registry::names()
        .filter_map(|name| name.strip_prefix("auth token "))
        .map(|rest| rest.split_once(' ').map_or(rest, |(verb, _)| verb))
        .collect()
}

/// The token-level help, listing the three verbs and their one-way secret rule.
///
/// This screen is authored prose, not a derived list: its verb lines are
/// display forms the records render differently (`add`'s line omits the
/// optional `--output`), and the frozen golden capture holds these bytes.
const TOKEN_HELP: &str = "\
usage: clio auth token <verb> [flags]

`token` verbs:
  clio auth token add --label LABEL --actor ACTOR --bank BANK [--bank BANK]
                        (no MCP tool)
  clio auth token list   (no MCP tool)
  clio auth token revoke --label LABEL   (no MCP tool)";

/// Dispatch `clio auth <subcommand>` with the process streams and environment.
pub(crate) fn auth(args: Vec<String>) -> ExitCode {
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

/// Resolve, parse, and run one `auth token` verb.
fn run(io: &mut CliIo, args: Vec<String>) -> ExitCode {
    let fallback = cli_output::mode_from_raw(&args, io.is_tty);
    let mut tokens = args.into_iter();
    let Some(sub) = tokens.next() else {
        return fail(
            io,
            fallback,
            &CliError::usage_hint("`auth` requires a subcommand", "try `clio auth token list`"),
        );
    };
    if sub == "--help" || sub == "-h" {
        let subs = clio_group_subcommands();
        let _ = cli_output::write_text(&mut io.out, &crate::cli_help::group_help("auth", subs));
        return ExitCode::from(EXIT_OK);
    }
    if !auth_subcommands().contains(&sub.as_str()) {
        return fail(io, fallback, &unknown_sub(&sub));
    }
    let Some(verb) = tokens.next() else {
        return fail(
            io,
            fallback,
            &CliError::usage_hint("`auth token` requires a verb", "try `clio auth token list`"),
        );
    };
    if verb == "--help" || verb == "-h" {
        let _ = cli_output::write_text(&mut io.out, TOKEN_HELP);
        return ExitCode::from(EXIT_OK);
    }
    if !token_verbs().contains(&verb.as_str()) {
        return fail(io, fallback, &unknown_verb(&verb));
    }
    let verb = format!("auth token {verb}");
    let parsed = match cli_args::parse(&spec(&verb), tokens) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Some(code) = help_if_requested(&verb, &parsed, io) {
        return code;
    }
    if let Err(error) = refuse_store_flags(&verb, &parsed) {
        return fail(io, fallback, &error);
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    let path = match store_path(io) {
        Ok(path) => path,
        Err(error) => return fail(io, mode, &error),
    };
    match verb.as_str() {
        "auth token add" => run_add(io, mode, &path, &parsed),
        "auth token list" => run_list(io, mode, &path, &parsed),
        _ => run_revoke(io, mode, &path, &parsed),
    }
}

fn clio_group_subcommands() -> &'static [&'static str] {
    crate::cli_help::group_subcommands("auth").unwrap_or(&[])
}

/// The error for an unknown `auth` subcommand or `token` verb.
fn unknown_sub(sub: &str) -> CliError {
    let hint = cli_args::suggest(sub, &auth_subcommands()).map_or_else(
        || "try `clio auth token list`".to_owned(),
        |best| format!("did you mean `clio auth {best}`?"),
    );
    CliError::usage(format!("unknown `auth` subcommand `{sub}`")).with_hint(hint)
}

fn unknown_verb(verb: &str) -> CliError {
    let hint = cli_args::suggest(verb, &token_verbs()).map_or_else(
        || "try `clio auth token list`".to_owned(),
        |best| format!("did you mean `clio auth token {best}`?"),
    );
    CliError::usage(format!("unknown `auth token` verb `{verb}`")).with_hint(hint)
}

/// The flags an `auth token` verb accepts: the verb's registry record's parser
/// spec, globals included. `add`'s `--bank` stays repeatable through the
/// record, one occurrence per bank.
fn spec(verb: &str) -> FlagSpec {
    match crate::cli_command_registry::find(verb) {
        Some(spec) => spec.parser_spec(),
        None => FlagSpec::new(&[], &[]),
    }
}

/// Render a verb's `--help`, before any store file is touched.
fn help_if_requested(verb: &str, parsed: &ParsedArgs, io: &mut CliIo) -> Option<ExitCode> {
    if !parsed.has("help") {
        return None;
    }
    let usage = crate::cli_help_usage::usage_for(verb).unwrap_or_else(|| TOKEN_HELP.to_owned());
    let _ = cli_output::write_text(&mut io.out, &usage);
    Some(ExitCode::from(EXIT_OK))
}

/// Refuse the global store flags this surface cannot honour.
///
/// Every `auth token` verb reads and writes one credential file, never a
/// database, so `--db` and `--backend` cannot change the answer. `--actor` is
/// the flag `add` reads, so it is refused on the two verbs that cannot use it.
/// `--bank` is `add`'s own flag and a no-op elsewhere. The parser unions the
/// globals into every spec (`FlagSpec::new`), so without this check each of
/// these would be accepted and silently ignored — the one option the sibling
/// archive surface explicitly rejects
/// (`crates/clio/src/backup_list_cli.rs::refuse_store_flags`).
fn refuse_store_flags(verb: &str, parsed: &ParsedArgs) -> Result<(), CliError> {
    let accepts = |flag: &str| match verb {
        "auth token add" => matches!(flag, "bank" | "actor"),
        _ => false,
    };
    for flag in ["db", "backend", "actor", "bank"] {
        if parsed.value(flag).is_some() && !accepts(flag) {
            return Err(CliError::usage(format!(
                "`clio {verb}` manages credentials in one file, not a store; `--{flag}` cannot \
                 change what it does"
            ))
            .with_hint("drop the flag, or use `clio setup --database` to choose a store"));
        }
    }
    Ok(())
}

/// Resolve the store path from the injected environment.
///
/// A blank `CLIO_AUTH_TOKEN_FILE` counts as unset, so the resolver falls
/// through to the XDG default rather than refusing here — the same reading
/// `config::persist` gives `CLIO_DEPLOYMENT_CONFIG`, and the reading the
/// store's own resolver implements. The refusal here is for the one case that
/// is genuinely unresolvable: no override and no config directory at all.
fn store_path(io: &CliIo) -> Result<std::path::PathBuf, CliError> {
    resolve_token_store_path_with(None, |key| (io.env)(key)).ok_or_else(|| {
        CliError::operational(
            "internal",
            "cannot resolve a config directory for the token store; \
             set CLIO_AUTH_TOKEN_FILE, or XDG_CONFIG_HOME/HOME so the default has somewhere to go",
        )
    })
}

/// `auth token add --label L --actor A --bank B [--bank B…]`.
///
/// The whole read-modify-write runs under the store's exclusive hold
/// (`clio_config::auth_tokens::add_token`), and the secret is printed only
/// after the save is confirmed: a token that could not be stored never prints
/// one.
fn run_add(
    io: &mut CliIo,
    mode: OutputMode,
    path: &std::path::Path,
    parsed: &ParsedArgs,
) -> ExitCode {
    if let Err(error) = crate::runtime_cli::no_positionals(parsed) {
        return fail(io, mode, &error);
    }
    let Some(label) = parsed.value("label") else {
        return fail(io, mode, &required_flag("--label", "add"));
    };
    let Some(actor) = parsed.value("actor") else {
        return fail(io, mode, &required_flag("--actor", "add"));
    };
    let banks: Vec<String> = parsed.values_of("bank").to_vec();
    if banks.is_empty() {
        return fail(io, mode, &required_flag("--bank", "add"));
    }
    let (record, secret) = match add_token(path, label, actor, &banks) {
        Ok(pair) => pair,
        Err(error) => return fail(io, mode, &CliError::from_am_error(&error)),
    };
    let payload = json!({
        "ok": true,
        "label": label,
        "actor": actor,
        "banks": record.banks,
        "token": secret,
        "store": path.display().to_string(),
        "note": "the secret is printed once and cannot be recovered; only its digest is stored",
    });
    let text = format!(
        "token `{label}` added (actor {actor}, banks: {})\n\
         {secret}\n\
         stored (digest only) in {}; the secret cannot be recovered — save it now",
        record.banks.join(", "),
        path.display()
    );
    emit(io, mode, &payload, &text);
    ExitCode::from(EXIT_OK)
}

/// The usage error for a missing required flag.
fn required_flag(flag: &str, verb: &str) -> CliError {
    CliError::usage_hint(
        format!("`auth token {verb}` requires {flag}"),
        format!("try `clio auth token {verb} --help`"),
    )
}

/// `auth token list`: labels, actors, banks, `created_at` — never a secret or
/// a digest, in either output mode.
fn run_list(
    io: &mut CliIo,
    mode: OutputMode,
    path: &std::path::Path,
    parsed: &ParsedArgs,
) -> ExitCode {
    if let Err(error) = crate::runtime_cli::no_positionals(parsed) {
        return fail(io, mode, &error);
    }
    let records = match load_auth_tokens(path) {
        Ok(records) => records,
        Err(error) => return fail(io, mode, &CliError::from_am_error(&error)),
    };
    let payload = json!({
        "ok": true,
        "store": path.display().to_string(),
        "records": records.iter().map(row).collect::<Vec<Value>>(),
    });
    let mut text = format!(
        "auth token store {} ({} record(s))",
        path.display(),
        records.len()
    );
    for record in &records {
        std::fmt::Write::write_fmt(
            &mut text,
            format_args!(
                "\n  {}: actor={} banks={} created_at={}",
                record.label,
                record.actor,
                record.banks.join(","),
                record.created_at
            ),
        )
        .expect("write list row to String");
    }
    emit(io, mode, &payload, &text);
    ExitCode::from(EXIT_OK)
}

/// The one `list` row: exactly the fields the verb may print.
fn row(record: &AuthTokenRecord) -> Value {
    json!({
        "label": record.label,
        "actor": record.actor,
        "banks": record.banks,
        "created_at": record.created_at,
    })
}

/// `auth token revoke --label L`; the change applies at the next server start.
///
/// The removal runs under the store's exclusive hold
/// (`clio_config::auth_tokens::revoke_token`), so a rewrite cannot drop a
/// record a concurrent `add` wrote after this one read.
fn run_revoke(
    io: &mut CliIo,
    mode: OutputMode,
    path: &std::path::Path,
    parsed: &ParsedArgs,
) -> ExitCode {
    if let Err(error) = crate::runtime_cli::no_positionals(parsed) {
        return fail(io, mode, &error);
    }
    let Some(label) = parsed.value("label") else {
        return fail(io, mode, &required_flag("--label", "revoke"));
    };
    if let Err(error) = revoke_token(path, label) {
        return fail(io, mode, &CliError::from_am_error(&error));
    }
    let payload = json!({
        "ok": true,
        "label": label,
        "effective": "next_server_start",
        "note": "servers load the token store at start, so revocation applies at the next server start",
    });
    let text = format!(
        "token `{label}` revoked; servers load the store at start, so the change \
         takes effect at the next server start"
    );
    emit(io, mode, &payload, &text);
    ExitCode::from(EXIT_OK)
}

#[cfg(test)]
#[path = "auth_cli_tests.rs"]
mod tests;
