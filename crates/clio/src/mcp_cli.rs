// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio mcp` subcommand handlers: schema
//! export, the stdio transport launcher, and the Streamable HTTP transport
//! launcher with their flag-to-options decoding.
//!
//! ## Owns
//! - `clio mcp schema-export|stdio|http` handlers and their usage text
//! - Decoding CLI flags and environment into `McpOpenOptions`
//! - The stdio session seam (`mcp_stdio_serve`) over injected streams
//!
//! ## Does not own
//! - The argument grammar (`parse_flags`, see the binary root)
//! - MCP protocol/tool semantics or transports themselves (see `clio-mcp`)
//! - The top-level help layout (see the binary root)
//!
//! ## Boundary
//!
//! Changes to a `clio mcp` subcommand's flags or lifecycle should normally
//! require changes here. Changes to MCP tool semantics should not require
//! changes here.
//!
//! Keep this module focused on the `clio mcp` subcommand handlers.

use super::cli_top_level::database_help_block;
use super::cli_top_level::parse_flags;
use std::process::ExitCode;

#[path = "mcp_cli_credentials.rs"]
mod mcp_cli_credentials;

/// `clio mcp` subcommands: schema export, stdio server, Streamable HTTP server.
pub(crate) fn mcp(args: impl IntoIterator<Item = String>) -> ExitCode {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("schema-export") => mcp_schema_export(args),
        Some("stdio") => mcp_stdio(args),
        Some("http") => mcp_http(args),
        Some("help" | "--help" | "-h") => {
            print_mcp_help();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown mcp command `{other}`; try `clio mcp stdio|http|schema-export`");
            ExitCode::from(2)
        }
        None => {
            eprintln!("missing mcp command; try `clio mcp stdio|http|schema-export`");
            ExitCode::from(2)
        }
    }
}

/// The `clio mcp` subcommands, one usage line each.
fn print_mcp_help() {
    for command in ["mcp schema-export", "mcp stdio", "mcp http"] {
        println!(
            "{}",
            crate::cli_help_usage::usage_line(command).unwrap_or_else(|| command.to_owned())
        );
    }
    println!("Ask a subcommand for its flags with `clio mcp <subcommand> --help`.");
}

pub(crate) fn mcp_schema_export(args: impl IntoIterator<Item = String>) -> ExitCode {
    let collected: Vec<String> = args.into_iter().collect();
    // `--help` used to fall through and print the whole schema pack, so asking
    // for help produced 70 KB of JSON and never said what the command takes.
    // The help screen is the record's: its usage line and its notes.
    if wants_help(&collected) {
        println!("{}", schema_export_help_text());
        return ExitCode::SUCCESS;
    }
    let names = crate::cli_command_registry_surface::parse_flag_names("mcp schema-export");
    let flags = match parse_flags(collected.iter().cloned(), &names) {
        Ok(flags) => flags,
        Err(error) => {
            return crate::cli_top_level::report_surface_usage_error(&error, &collected);
        }
    };
    let pack = clio_mcp::schema::schema_pack();
    // `--path` is accepted as a documented alias for `--out`.
    if let Some(path) = flags.get("out").or_else(|| flags.get("path")) {
        return match std::fs::write(path, serde_json::to_vec_pretty(&pack).expect("serialize")) {
            Ok(()) => {
                println!("wrote {path}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("write failed: {e}");
                ExitCode::from(1)
            }
        };
    }
    println!("{}", serde_json::to_string(&pack).expect("serialize"));
    ExitCode::SUCCESS
}

/// The `schema-export --help` screen as text, split from the handler so a test
/// can read it without capturing stdout (the `help_text` split in `install_cli`).
/// It renders the record's usage line and notes, never the schema pack, so the
/// handler printing this string is what keeps the old pack fall-through fixed.
pub(crate) fn schema_export_help_text() -> String {
    let spec = crate::cli_command_registry::find("mcp schema-export")
        .expect("`mcp schema-export` is registered");
    format!("{}\n\n{}", spec.usage_line(), spec.notes.join("\n\n"))
}

pub(crate) fn mcp_options(
    args: impl IntoIterator<Item = String>,
) -> Result<clio_mcp::runtime::McpOpenOptions, String> {
    mcp_options_with(args, &|key| std::env::var(key).ok())
}

/// Build MCP options through the shared resolver with an injectable
/// environment lookup (tests never depend on the host env).
pub(crate) fn mcp_options_with<F>(
    args: impl IntoIterator<Item = String>,
    get: &F,
) -> Result<clio_mcp::runtime::McpOpenOptions, String>
where
    F: Fn(&str) -> Option<String>,
{
    // The shared builder parses the http record's name list, the wider of the
    // two transport sets: `mcp stdio` pre-validates its own narrower record
    // set before it calls in, so a typo there fails closed with exit 2 while
    // this builder's refusal is an open failure.
    let flags = match parse_flags(
        args,
        &crate::cli_command_registry_surface::parse_flag_names("mcp http"),
    ) {
        Ok(flags) => flags,
        Err(error) => return Err(error.message().to_owned()),
    };
    let resolved = crate::db_target::resolve_target(
        get,
        flags.get("db").map(String::as_str),
        flags.get("backend").map(String::as_str),
    )?;
    let kms_path = clio_config::kms_path::resolve_kms_path(get, &resolved.url);
    Ok(clio_mcp::runtime::McpOpenOptions {
        backend: resolved.backend,
        database_url: resolved.url,
        // The MCP server's `--bank` is its configured default: the bank a
        // request that names none resolves to. A request that names another
        // bank is still checked against the store, so exempting this default
        // is safe.
        default_bank: flags
            .get("bank")
            .cloned()
            .unwrap_or_else(|| "default".into()),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: flags.contains_key("shared-bank"),
        kms_path,
    })
}

/// True when `--help` / `-h` appears anywhere in a subcommand's arguments.
pub(crate) fn wants_help(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--help" || arg == "-h")
}

fn mcp_stdio(args: impl IntoIterator<Item = String>) -> ExitCode {
    let collected: Vec<String> = args.into_iter().collect();
    if wants_help(&collected) {
        println!(
            "{}\n\n{}",
            crate::cli_help_usage::usage_line("mcp stdio")
                .unwrap_or_else(|| "clio mcp stdio".to_owned()),
            database_help_block(&|key| std::env::var(key).ok())
        );
        return ExitCode::SUCCESS;
    }
    // A flag this transport does not take is a usage error (exit 2), checked
    // before the store is opened, so `clio mcp stdio --bogus` fails closed the
    // way every other surface does rather than reporting an exit-1 open
    // failure. The names are the stdio record's own, the narrower of the two
    // transport sets; the shared builder below parses the http record's wider
    // one and reports only open failures.
    let names = crate::cli_command_registry_surface::parse_flag_names("mcp stdio");
    if let Err(error) = parse_flags(collected.iter().cloned(), &names) {
        return crate::cli_top_level::report_surface_usage_error(&error, &collected);
    }
    let opts = match mcp_options(collected) {
        Ok(opts) => opts,
        Err(reason) => {
            eprintln!("open failed: {reason}");
            return ExitCode::from(1);
        }
    };
    // A long-running transport must outlive one client: with the startup default
    // in place, a dropped client would end the session with a signal rather than
    // a recoverable write error. The witness is handed to the serve entry point,
    // so serving without installing the disposition does not compile.
    let serving = crate::sigpipe::begin_serving();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    mcp_stdio_serve(&serving, &opts, stdin.lock(), &mut stdout)
}

/// Open the store and serve one MCP stdio session over injected streams.
///
/// Split out from [`mcp_stdio`] so the session path is testable without a TTY.
/// The `Serving` witness is what proves `SIGPIPE` was ignored before the
/// session starts; only [`crate::sigpipe::begin_serving`] can make one.
pub(crate) fn mcp_stdio_serve<R: std::io::BufRead, W: std::io::Write>(
    _serving: &crate::sigpipe::Serving,
    opts: &clio_mcp::runtime::McpOpenOptions,
    input: R,
    output: &mut W,
) -> ExitCode {
    match clio_mcp::runtime::McpState::open(opts) {
        Ok(state) => {
            // Background index drain: drains the durable outbox off the
            // response path (post-write nudge + periodic sweep for retries).
            let _sweeper = state.spawn_index_sweeper();
            let handler = clio_mcp::protocol::McpHandler::new(state);
            match clio_mcp::stdio::serve(&handler, input, output) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("mcp stdio error: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Err(e) => {
            eprintln!("open failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn mcp_http(args: impl IntoIterator<Item = String>) -> ExitCode {
    let collected: Vec<String> = args.into_iter().collect();
    if wants_help(&collected) {
        println!("{}", mcp_http_help());
        return ExitCode::SUCCESS;
    }
    let names = crate::cli_command_registry_surface::parse_flag_names("mcp http");
    let flags = match parse_flags(collected.iter().cloned(), &names) {
        Ok(flags) => flags,
        Err(error) => return crate::cli_top_level::report_surface_usage_error(&error, &collected),
    };
    if let Err(error) = refuse_sessions_value(&flags) {
        return crate::cli_top_level::report_surface_usage_error(&error, &collected);
    }
    let opts = match mcp_options(collected.iter().cloned()) {
        Ok(opts) => opts,
        Err(reason) => {
            eprintln!("open failed: {reason}");
            return ExitCode::from(1);
        }
    };
    // The credential store is loaded once here, before the listener is bound, so
    // an unusable path refuses the start instead of serving a 401 for every
    // request. Fail closed on every unusable state, including a store with no
    // record: it would authorize nobody.
    let credentials =
        match mcp_cli_credentials::load_http_credentials(&flags, &|key| std::env::var(key).ok()) {
            Ok(credentials) => credentials,
            Err(reason) => {
                eprintln!("mcp http error: {reason}");
                return ExitCode::from(1);
            }
        };
    let mut cfg = http_config(&flags);
    cfg.credentials = credentials;
    // As for the stdio transport: the server must outlive one client going away.
    let serving = crate::sigpipe::begin_serving();
    match clio_mcp::runtime::McpState::open(&opts) {
        Ok(state) => {
            // Same background index drain as the stdio transport.
            let _sweeper = state.spawn_index_sweeper();
            match crate::http_bind::resolve_listener(flags.get("bind").map(String::as_str)) {
                Ok(listener) => {
                    let addr = listener
                        .local_addr()
                        .expect("bound listener has an address");
                    // `--insecure-no-auth` is the one opt-out. Without it every
                    // bind needs a credential: a token store named by
                    // `--token-file`. A loopback bind with no token store is
                    // refused here, so the operator creates a credential with
                    // `clio auth token add` rather than receiving a secret on
                    // stderr, and a rejected bind is not announced as listening.
                    if !clio_mcp::http::has_credential_source(&cfg) && !cfg.insecure_no_auth {
                        eprintln!(
                            "mcp http error: bind requires an auth token: point \
                             --token-file at a store holding at least one record \
                             (see `clio auth token add`), or pass --insecure-no-auth \
                             to serve without authentication"
                        );
                        return ExitCode::from(1);
                    }
                    cfg.bind = addr.to_string();
                    eprintln!("mcp http listening on {addr}");
                    match mcp_http_serve(&serving, state, cfg, listener) {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(e) => {
                            eprintln!("mcp http error: {e}");
                            ExitCode::from(1)
                        }
                    }
                }
                Err(e) => {
                    eprintln!("mcp http error: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Err(e) => {
            eprintln!("open failed: {e}");
            ExitCode::from(1)
        }
    }
}

/// Serve the HTTP transport under the disposition `begin_serving` installed.
///
/// The witness argument ties this serve call to
/// [`crate::sigpipe::begin_serving`]: the transport cannot be started without
/// it, so the disposition line cannot be dropped without a compile error.
fn mcp_http_serve(
    _serving: &crate::sigpipe::Serving,
    state: std::sync::Arc<clio_mcp::runtime::McpState>,
    cfg: clio_mcp::http::HttpConfig,
    listener: std::net::TcpListener,
) -> std::io::Result<()> {
    clio_mcp::http::serve_with_listener(state, cfg, listener)
}

/// Build the HTTP transport config from CLI flags (testable without a server).
///
/// `bind` is set later in [`mcp_http`] to the address actually bound, so the
/// config handed to the transport always names the served address. The
/// credential store is not assembled here because loading it can fail; see the
/// `mcp_cli_credentials` module's `load_http_credentials`.
pub(crate) fn http_config(
    flags: &std::collections::HashMap<String, String>,
) -> clio_mcp::http::HttpConfig {
    clio_mcp::http::HttpConfig {
        sessions_enabled: flags.contains_key("sessions"),
        insecure_no_auth: flags
            .get("insecure-no-auth")
            .is_some_and(|value| value == "true"),
        ..clio_mcp::http::HttpConfig::default()
    }
}

/// `--sessions` is a switch. Bare, it turns sessions on. With a value
/// (`--sessions=false`, or a token taken as `--sessions extra`) the value is
/// not what the caller meant, and sessions would still turn on, so it fails
/// closed instead.
fn refuse_sessions_value(
    flags: &std::collections::HashMap<String, String>,
) -> Result<(), crate::cli_error::CliError> {
    match flags.get("sessions") {
        Some(value) if value != "true" => Err(crate::cli_error::CliError::usage_hint(
            "--sessions takes no value",
            "pass --sessions alone to turn sessions on, or leave it out",
        )),
        _ => Ok(()),
    }
}

/// `clio mcp http` usage text, including the auto-bind rule.
pub(crate) fn mcp_http_help() -> String {
    let range = format!(
        "{}-{}",
        crate::port_scan::FIRST_PORT,
        crate::port_scan::LAST_PORT
    );
    format!(
        "{}\n\n--bind ADDR  Bind address. With no flag, or with a bare `--bind`, scan 127.0.0.1:{range}
             for the first free port. An explicit `--bind HOST:PORT` binds exactly
             that address; a non-loopback address also requires a credential.

--token-file PATH
             Credential store whose records name an actor and the banks it may reach
             (see `clio auth token add`). Loaded once at start; a path that is
             absent, unreadable, malformed, or holds no record refuses the start.
             Required on every bind unless --insecure-no-auth is given.

--insecure-no-auth
             Permit a bind with no credential. Without it a credential is
             required everywhere, including loopback: name a --token-file store,
             or the start is refused. A --token-file store, when given, is still
             enforced.

{}",
        crate::cli_help_usage::usage_line("mcp http")
            .unwrap_or_else(|| "clio mcp http".to_owned()),
        database_help_block(&|key| std::env::var(key).ok())
    )
}

#[cfg(test)]
#[path = "mcp_cli_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "mcp_cli_dispatch_tests.rs"]
mod dispatch_tests;

#[cfg(test)]
#[path = "mcp_cli_sessions_tests.rs"]
mod sessions_tests;

#[cfg(test)]
#[path = "mcp_cli_insecure_tests.rs"]
mod insecure_tests;
