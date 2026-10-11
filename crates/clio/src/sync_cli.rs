// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio sync` CLI surface: masked local
//! status, push/pull against a remote peer, and serving the sync endpoint, with
//! a single-host explanation for `sync status` instead of an obscure failure.
//!
//! ## Owns
//! - The `sync status|push|pull|serve` verbs, their flags, and their text views
//! - The single-host detection (`sync.omit`) and explanation path
//! - The serve lifecycle: emit the bound address, then hold the process for the
//!   resolved TTL so the background server is reachable
//!
//! ## Does not own
//! - Sync protocol, apply matrix, conflict rule, or auth (`clio-sync`)
//! - The in-process call bridge or output/error contracts (see `cli_read`)
//! - The database precedence resolver (see `clio_config::db_path`)
//!
//! ## Boundary
//!
//! Changes to the sync CLI flags or views should normally require changes here.
//! Changes to sync behavior should not require changes here.
//!
//! Keep this module focused on the `clio sync` surface.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_help;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, call_tool, emit, fail};
use crate::sync_cli_render::render;
use clio::Runtime;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Map, Value, json};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

/// The sync subcommands this surface binds.
const SUBCOMMANDS: &[&str] = &["status", "push", "pull", "serve"];

/// Dispatch `clio sync <subcommand>`.
pub(crate) fn sync(args: Vec<String>) -> ExitCode {
    // The key-management verbs own their own module and their own flag spec, so
    // they are routed before this module's whitelist and parser. Routing them
    // through `spec()`'s catch-all would hand `keygen` the serve flags.
    if matches!(args.first().map(String::as_str), Some("keygen" | "key")) {
        return crate::sync_key_cli::sync_key(args);
    }
    // `sync serve` holds the process open for the resolved TTL so the background
    // server stays reachable; a dropped peer must not end it with a signal. The
    // witness is threaded into the hold-open path, so serving without installing
    // the disposition does not compile.
    let serving = if matches!(args.first().map(String::as_str), Some("serve")) {
        Some(crate::sigpipe::begin_serving())
    } else {
        None
    };
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
    run(&mut io, args, config_single_host(), None, serving.as_ref())
}

/// True when the effective config omits sync (the single-host default).
///
/// One definition, in `setup_sync_config`, shared with the wizard's
/// `sync_in_play`, so the two cannot disagree about what a single-host
/// deployment is.
fn config_single_host() -> bool {
    crate::setup_sync_config::single_host(&Runtime::new().effective_raw())
}

/// Resolve, parse, and run one `sync` verb.
///
/// `injected` is the test seam: when `Some`, the in-process dispatcher runs
/// against that state instead of opening the database.
fn run(
    io: &mut CliIo,
    args: Vec<String>,
    single_host: bool,
    injected: Option<&Arc<McpState>>,
    serving: Option<&crate::sigpipe::Serving>,
) -> ExitCode {
    let fallback = cli_output::mode_from_raw(&args, io.is_tty);
    let mut tokens = args.into_iter();
    let Some(sub) = tokens.next() else {
        return fail(
            io,
            fallback,
            &CliError::usage_hint("`sync` requires a subcommand", "try `clio sync status`"),
        );
    };
    if sub == "--help" || sub == "-h" {
        let subs = cli_help::group_subcommands("sync").unwrap_or(&[]);
        let _ = cli_output::write_text(&mut io.out, &cli_help::group_help("sync", subs));
        return ExitCode::from(EXIT_OK);
    }
    if !SUBCOMMANDS.contains(&sub.as_str()) {
        return fail(io, fallback, &unknown_subcommand(&sub));
    }
    let verb = format!("sync {sub}");
    let parsed = match cli_args::parse(&spec(&verb), tokens) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Some(code) = help_if_requested(&verb, &parsed, io) {
        return code;
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    // Decode flags before the single-host short-circuit: usage errors must be
    // identical across deployment modes (a bad `--banks` exits 2 either way).
    let call = match build(&verb, &parsed, io.env) {
        Ok(call) => call,
        Err(error) => return fail(io, mode, &error),
    };
    if sub == "status" && single_host {
        return emit_single_host(io, mode);
    }
    let (tool, tool_args) = call;
    let state = match injected {
        Some(state) => Arc::clone(state),
        None => match open_state(io, &parsed) {
            Ok(state) => state,
            Err(error) => return fail(io, mode, &error),
        },
    };
    match call_tool(&state, tool, &tool_args) {
        Ok(payload) => {
            emit(io, mode, &payload, &render(&verb, &payload));
            if verb == "sync serve" {
                let ttl = payload.get("ttl_secs").and_then(Value::as_u64).unwrap_or(0);
                wait_for_ttl(
                    Duration::from_secs(ttl),
                    serving.expect("only the serve verb reaches the hold-open wait"),
                );
            }
            ExitCode::from(EXIT_OK)
        }
        Err(error) => fail(io, mode, &error),
    }
}

/// The error for an unknown subcommand, suggesting the closest verb.
fn unknown_subcommand(sub: &str) -> CliError {
    let hint = cli_args::suggest(sub, SUBCOMMANDS).map_or_else(
        || "try `clio sync status`".to_owned(),
        |best| format!("did you mean `clio sync {best}`?"),
    );
    CliError::usage(format!("unknown `sync` subcommand `{sub}`")).with_hint(hint)
}

/// The flags a sync verb accepts, globals included.
fn spec(verb: &str) -> FlagSpec {
    match verb {
        "sync status" => FlagSpec::new(&["banks"], &[]),
        "sync push" | "sync pull" => FlagSpec::new(&["remote", "mode", "banks", "token"], &[]),
        _ => FlagSpec::new(&["bind", "banks", "token-file", "ttl-secs"], &["dev"]),
    }
}

/// Render a verb's `--help`, before any database is opened.
fn help_if_requested(verb: &str, parsed: &ParsedArgs, io: &mut CliIo) -> Option<ExitCode> {
    if !parsed.has("help") {
        return None;
    }
    let (usage, tool) = cli_help::verb_help(verb)?;
    let _ = cli_output::write_text(
        &mut io.out,
        &format!("{usage}\n\n{verb} calls the `{tool}` tool."),
    );
    Some(ExitCode::from(EXIT_OK))
}

/// Open the in-process state through the shared database precedence resolver.
fn open_state(io: &CliIo, parsed: &ParsedArgs) -> Result<Arc<McpState>, CliError> {
    let get = |key: &str| (io.env)(key);
    let resolved =
        crate::db_target::resolve_target(io.env, parsed.value("db"), parsed.value("backend"))
            .map_err(CliError::usage)?;
    let kms_path = clio_config::kms_path::resolve_kms_path(&get, &resolved.url);
    let options = McpOpenOptions {
        backend: resolved.backend,
        database_url: resolved.url,
        // Sync transfer treats `--bank` as the operation's configured bank: the
        // transfer reads or creates it directly, so it is not run through the
        // read-side known-bank check.
        default_bank: parsed.value("bank").unwrap_or("default").to_owned(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path,
    };
    McpState::open_one_shot(&options).map_err(|error| CliError::from_am_error(&error))
}

/// Decode a sync verb into its tool name and arguments.
fn build(
    verb: &str,
    parsed: &ParsedArgs,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(&'static str, Value), CliError> {
    match verb {
        "sync status" => Ok(("sync_status", Value::Object(status_args(parsed)?))),
        "sync push" | "sync pull" => transfer_args(verb, parsed, env),
        _ => serve_args(parsed),
    }
}

/// `sync status [--banks a,b]`.
fn status_args(parsed: &ParsedArgs) -> Result<Map<String, Value>, CliError> {
    crate::runtime_cli::no_positionals(parsed)?;
    let mut args = Map::new();
    if let Some(banks) = banks_arg(parsed)? {
        args.insert("banks".into(), banks);
    }
    Ok(args)
}

/// `sync push|pull --remote HOST:PORT [--mode M] [--banks a,b] [--token T]`.
///
/// The cipher key is not a CLI argument. The sync tool reads the recorded key
/// store, or the file `CLIO_SYNC_KEY_FILE` names, when no key is passed.
fn transfer_args(
    verb: &str,
    parsed: &ParsedArgs,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(&'static str, Value), CliError> {
    crate::runtime_cli::no_positionals(parsed)?;
    let remote = parsed.value("remote").ok_or_else(|| {
        CliError::usage_hint(
            format!("`{verb}` requires --remote HOST:PORT"),
            format!("try `clio {verb} --remote 127.0.0.1:34311`"),
        )
    })?;
    let mut args = Map::new();
    args.insert("remote".into(), Value::from(remote));
    if let Some(mode) = parsed.value("mode") {
        let mode =
            crate::cli_read::validate_choice("mode", mode, &["push", "pull", "bidirectional"])?;
        args.insert("mode".into(), Value::from(mode));
    }
    if let Some(banks) = banks_arg(parsed)? {
        args.insert("banks".into(), banks);
    }
    if let Some(token) = secret_flag(parsed, "token", "CLIO_SYNC_TOKEN", env) {
        args.insert("token".into(), Value::from(token));
    }
    let tool = if verb == "sync push" {
        "sync_push"
    } else {
        "sync_pull"
    };
    Ok((tool, Value::Object(args)))
}

/// `sync serve [--bind ADDR] [--banks a,b] [--token-file PATH] [--ttl-secs N]
/// [--dev]`.
fn serve_args(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    crate::runtime_cli::no_positionals(parsed)?;
    let mut args = Map::new();
    if let Some(bind) = parsed.value("bind") {
        args.insert("bind".into(), Value::from(bind));
    }
    if let Some(banks) = banks_arg(parsed)? {
        args.insert("banks".into(), banks);
    }
    if let Some(path) = parsed.value("token-file") {
        args.insert("token_file".into(), Value::from(path));
    }
    if let Some(ttl) = parsed.value("ttl-secs") {
        args.insert(
            "ttl_secs".into(),
            Value::from(crate::cli_read::parse_u64("ttl-secs", ttl)?),
        );
    }
    if parsed.has("dev") {
        args.insert("dev".into(), Value::Bool(true));
    }
    Ok(("sync_serve", Value::Object(args)))
}

/// Decode `--banks a,b` into a JSON array of non-empty trimmed names.
fn banks_arg(parsed: &ParsedArgs) -> Result<Option<Value>, CliError> {
    let Some(raw) = parsed.value("banks") else {
        return Ok(None);
    };
    let banks: Vec<Value> = raw
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(Value::from)
        .collect();
    if banks.is_empty() {
        return Err(CliError::usage_hint(
            "--banks must name at least one bank",
            "try `--banks demo`",
        ));
    }
    Ok(Some(Value::Array(banks)))
}

/// A secret flag value from the flag, else its documented environment variable.
fn secret_flag(
    parsed: &ParsedArgs,
    flag: &str,
    env_var: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    parsed
        .value(flag)
        .map(str::to_owned)
        .or_else(|| env(env_var).filter(|value| !value.is_empty()))
}

/// Emit the single-host explanation and exit 0.
fn emit_single_host(io: &mut CliIo, mode: OutputMode) -> ExitCode {
    let explanation = "sync is omitted on this single-host deployment by design \
        (the single-host exception); the local store is authoritative and no \
        sync peer is configured. Set `sync.omit` to false for a multi-host deployment.";
    let payload = json!({
        "ok": true,
        "single_host": true,
        "sync": "omitted",
        "explanation": explanation,
    });
    emit(io, mode, &payload, explanation);
    ExitCode::from(EXIT_OK)
}

/// Hold the serve process for the resolved TTL so the server stays reachable.
fn wait_for_ttl(ttl: Duration, _serving: &crate::sigpipe::Serving) {
    if !ttl.is_zero() {
        std::thread::sleep(ttl);
    }
}

#[cfg(test)]
#[path = "sync_cli_tests.rs"]
mod tests;
