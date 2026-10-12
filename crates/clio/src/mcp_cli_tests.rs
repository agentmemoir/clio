// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio mcp` subcommand surface: dispatch, schema export,
//! flag-to-options decoding, the stdio session, and the usage text. It also
//! holds the fixtures the sibling credential-store suite borrows.
//!
//! ## Owns
//! - Dispatch and exit-code tests for `schema-export`, `stdio` and `http`
//! - Help-text tests naming the token-store source and showing that help never
//!   echoes a planted secret value
//! - The scratch fixture and the injected-lookup helpers both suites use
//!
//! ## Does not own
//! - The command implementation under test (`mcp_cli`)
//! - The credential-store load tests (`mcp_cli_credentials_tests`)
//! - The flag grammar (`parse_flags`) and the binary dispatch (`main`)
//! - MCP protocol, tool semantics, and the transports (`clio-mcp`)
//!
//! ## Boundary
//!
//! Tests here break when a `clio mcp` flag, its exit code, or the help wording
//! changes; they never change it.
//!
//! Keep this module focused on the `clio mcp` subcommand surface.

use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Calls served so far in this test binary, so no two tests share a scratch name.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One unique scratch path per call, removed on drop of the guard.
///
/// `tempfile` is not a dependency of this crate, so this is the workspace's own
/// fixture pattern: a unique path under the OS temp directory plus an explicit
/// cleanup, which also runs while a panic unwinds. The pid and a monotonic
/// counter keep parallel tests apart; the nanos keep two runs apart.
pub(crate) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "am-mcp-cli-{name}-{}-{nanos}-{sequence}.json",
            std::process::id()
        ));
        Self { path }
    }

    /// Write a credential store the loader will accept: owner-only, because
    /// the loader refuses a file another user can read before it parses it.
    pub(crate) fn write_store(&self, records: &[clio_config::auth_tokens::AuthTokenRecord]) {
        clio_config::auth_tokens::save_auth_tokens(&self.path, records).expect("save the store");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).expect("tighten");
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The path as an argument value.
    pub(crate) fn display(&self) -> String {
        self.path.display().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// One credential record, carrying the digest the writer would have produced.
pub(crate) fn record(label: &str, secret: &str) -> clio_config::auth_tokens::AuthTokenRecord {
    clio_config::auth_tokens::AuthTokenRecord {
        label: label.to_owned(),
        secret_sha256: clio_config::auth_tokens::digest_hex(secret),
        actor: clio_types::Actor::Agent,
        banks: vec!["default".to_owned()],
        created_at: "2026-10-01T00:00:00Z".into(),
    }
}

/// The `--token-file` flags, for a path a test already holds as a string.
pub(crate) fn token_file(path: &str) -> std::collections::HashMap<String, String> {
    flags_of(&["--token-file", path])
}

/// A path under the temp directory that is never created, for the cases that
/// must fail because the thing they name does not exist.
pub(crate) fn absent_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "am-mcp-cli-{tag}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

/// A lookup over an explicit table; the tests never touch the host
/// environment (no `std::env::set_var`, no `std::env::var`).
pub(crate) fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

pub(crate) fn flags_of(flags: &[&str]) -> std::collections::HashMap<String, String> {
    crate::cli_flags_test_support::flags_of(flags)
}

pub(crate) fn config_with(flags: &[&str]) -> clio_mcp::http::HttpConfig {
    http_config(&flags_of(flags))
}

pub(crate) fn args_of(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

pub(crate) fn opts_with(
    args: Vec<String>,
    pairs: &[(&str, &str)],
) -> clio_mcp::runtime::McpOpenOptions {
    mcp_options_with(args, &lookup(pairs)).expect("options resolve")
}

/// An in-memory target, so every assertion is decided by the flags a test
/// passes rather than by whatever the host environment exports.
pub(crate) fn memory(extra: &[&str]) -> Vec<String> {
    let mut all = args_of(&["--db", "sqlite::memory:"]);
    all.extend(args_of(extra));
    all
}

// ------------------------------------------------------------------ dispatch

#[test]
fn no_subcommand_is_an_unknown_mcp_command() {
    assert_eq!(mcp(Vec::<String>::new()), ExitCode::from(2));
}

#[test]
fn an_unknown_subcommand_exits_2() {
    assert_eq!(mcp(args_of(&["bogus"])), ExitCode::from(2));
}

#[test]
fn schema_export_prints_the_pack_to_stdout_without_a_flag() {
    assert_eq!(mcp(args_of(&["schema-export"])), ExitCode::SUCCESS);
}

#[test]
fn schema_export_writes_a_pretty_pack_to_the_out_flag() {
    let scratch = Scratch::new("out");
    let code = mcp_schema_export(args_of(&["--out", &scratch.display()]));
    assert_eq!(code, ExitCode::SUCCESS);
    let raw = fs::read_to_string(scratch.path()).expect("written pack");
    let pack: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(pack["format"], "am_tool_schema_pack");
    assert_eq!(pack["mcp_protocol_revision"], "2025-11-25");
}

#[test]
fn schema_export_accepts_path_as_an_alias_for_out() {
    let scratch = Scratch::new("path");
    let code = mcp_schema_export(args_of(&["--path", &scratch.display()]));
    assert_eq!(code, ExitCode::SUCCESS);
    let raw = fs::read_to_string(scratch.path()).expect("written pack");
    let pack: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(pack["format"], "am_tool_schema_pack");
}

#[test]
fn out_wins_when_both_out_and_path_are_given() {
    let out = Scratch::new("both-out");
    let path = Scratch::new("both-path");
    let flags = args_of(&["--out", &out.display(), "--path", &path.display()]);
    assert_eq!(mcp_schema_export(flags), ExitCode::SUCCESS);
    assert!(out.path().is_file(), "--out was not honoured");
    assert!(!path.path().exists(), "--path overrode --out");
}

#[test]
fn schema_export_into_a_missing_directory_exits_1() {
    // The parent directory is never created, so the write cannot succeed.
    let target = absent_path("absent-dir").join("out.json");
    let code = mcp_schema_export(args_of(&["--out", &target.display().to_string()]));
    assert_eq!(code, ExitCode::from(1));
    assert!(!target.exists());
}

#[test]
fn stdio_help_exits_zero() {
    assert_eq!(mcp(args_of(&["stdio", "--help"])), ExitCode::SUCCESS);
}

#[test]
fn http_help_exits_zero() {
    assert_eq!(mcp(args_of(&["http", "-h"])), ExitCode::SUCCESS);
}

#[test]
fn a_target_no_store_can_open_stops_both_transports_before_they_serve() {
    // An explicit `--backend` reaches the opener unchanged, so the refusal
    // comes from trying to open it rather than from flag decoding.
    for transport in ["stdio", "http"] {
        let code = mcp(args_of(&[transport, "--backend", "mysql"]));
        assert_eq!(code, ExitCode::from(1), "{transport} served anyway");
    }
}

#[test]
fn http_refuses_to_start_on_a_token_file_it_cannot_load() {
    let missing = absent_path("no-store");
    let mut args = args_of(&["http"]);
    args.extend(memory(&["--token-file", &missing.display().to_string()]));
    assert_eq!(mcp(args), ExitCode::from(1));
    assert!(!missing.exists());
}

#[test]
fn http_refuses_to_start_on_a_bind_it_cannot_bind() {
    // `999.1.1.1` is not an address this host owns, so the bind fails before
    // anything is served.
    let mut args = args_of(&["http"]);
    args.extend(memory(&["--bind", "999.1.1.1:1"]));
    assert_eq!(mcp(args), ExitCode::from(1));
}

// ------------------------------------------------------------------- options

#[test]
fn the_db_flag_beats_an_environment_url() {
    let opts = opts_with(
        args_of(&["--db", "./x.db"]),
        &[("CLIO_DATABASE_URL", "postgres://h/db")],
    );
    assert_eq!(opts.database_url, "./x.db");
    assert_eq!(opts.backend, "sqlite");
}

#[test]
fn an_environment_url_names_the_postgres_backend() {
    let opts = opts_with(args_of(&[]), &[("CLIO_DATABASE_URL", "postgres://h/db")]);
    assert_eq!(opts.database_url, "postgres://h/db");
    assert_eq!(opts.backend, "postgres");
}

#[test]
fn an_explicit_backend_beats_the_one_inferred_from_the_url() {
    let opts = opts_with(
        args_of(&["--backend", "sqlite"]),
        &[("CLIO_DATABASE_URL", "postgres://h/db")],
    );
    assert_eq!(opts.backend, "sqlite");
}

#[test]
fn a_configured_address_naming_a_server_this_build_cannot_open_is_refused() {
    // An explicit `--backend` is passed straight through, so the refusal comes
    // from the configured address instead: a scheme that is neither a
    // PostgreSQL URL nor a SQLite path.
    let reason = mcp_options_with(
        args_of(&[]),
        &lookup(&[("CLIO_DATABASE_URL", "mysql://h/db")]),
    )
    .expect_err("a mysql:// address is not a store this build opens");
    assert!(
        reason.contains("mysql://h/db"),
        "the reason must name it: {reason}"
    );
}

#[test]
fn the_data_dir_environment_variable_names_the_default_file() {
    let opts = opts_with(args_of(&[]), &[("CLIO_DATA_DIR", "/data")]);
    assert_eq!(opts.database_url, "/data/clio.db");
    assert_eq!(opts.backend, "sqlite");
}

#[test]
fn the_bank_flag_names_the_default_bank_and_agent_is_the_default_actor() {
    let opts = opts_with(memory(&["--bank", "bank-a"]), &[]);
    assert_eq!(opts.default_bank, "bank-a");
    assert_eq!(opts.default_actor, clio_types::Actor::Agent);
}

#[test]
fn the_default_bank_is_default_and_the_shared_bank_is_off_without_their_flags() {
    let opts = opts_with(memory(&[]), &[]);
    assert_eq!(opts.default_bank, "default");
    assert!(!opts.shared_bank_enabled);
}

#[test]
fn the_shared_bank_flag_turns_the_shared_surface_on() {
    assert!(opts_with(memory(&["--shared-bank"]), &[]).shared_bank_enabled);
}

#[test]
fn an_undeclared_flag_fails_the_shared_options_builder_with_the_flag_named() {
    // The handlers pre-validate their own flag sets, so this refusal is the
    // builder's own fail-closed contract; the reason names the flag so a typo
    // is diagnosable at either layer.
    let reason = mcp_options_with(args_of(&["--bogus"]), &lookup(&[]))
        .expect_err("an unknown flag is refused");
    assert!(
        reason.contains("--bogus"),
        "the reason must name it: {reason}"
    );
}

// --------------------------------------------------------------- help tests

#[test]
fn wants_help_spots_both_spellings_anywhere_in_the_arguments() {
    assert!(wants_help(&args_of(&["--bank", "b", "--help"])));
    assert!(wants_help(&args_of(&["-h"])));
    assert!(!wants_help(&args_of(&["--bank", "b"])));
    assert!(!wants_help(&args_of(&["--helped"])));
}

#[test]
fn http_help_names_the_scanned_port_range_from_the_shared_constants() {
    let help = mcp_http_help();
    let (first, last) = (crate::port_scan::FIRST_PORT, crate::port_scan::LAST_PORT);
    let range = format!("{first}-{last}");
    assert!(help.contains(&range), "range missing: {help}");
    assert!(help.contains("bare `--bind`"), "auto rule missing: {help}");
    assert!(help.contains("HOST:PORT"), "override missing: {help}");
}

#[test]
fn http_help_names_the_store_source_without_printing_a_secret_value() {
    // Plant a secret in parsed flags, then build the help: the help names the
    // store source but must never echo a secret value, and the removed flag and
    // environment sources must not be advertised.
    let secret = "planted-secret-token-value";
    let _flags = flags_of(&["--token-file", secret]);
    let help = mcp_http_help();
    assert!(
        !help.contains("CLIO_MCP_TOKEN"),
        "the removed environment source is still advertised: {help}"
    );
    assert!(
        !help.contains("--auth-token"),
        "the removed flag is still advertised: {help}"
    );
    assert!(
        help.contains("--token-file"),
        "store source missing: {help}"
    );
    assert!(!help.contains(secret), "help printed the secret: {help}");
}

#[test]
fn http_config_reads_the_sessions_flag_and_leaves_the_bind_to_the_listener() {
    let cfg = config_with(&["--sessions"]);
    assert!(cfg.sessions_enabled);
    // The address actually bound is filled in by the serving path, so the
    // config keeps the transport default until a listener is chosen.
    assert_eq!(cfg.bind, "127.0.0.1:0");
    assert!(cfg.credentials.is_none());
}

// The credential-store load has its own file, because a module's own tests
// are not limited to one file: see `mcp_cli_credentials_tests`.

// ------------------------------------------------------------ stdio session

/// An output stream that refuses every write, so the transport's write error
/// reaches the caller instead of a real terminal.
struct RefusesWrites;

impl std::io::Write for RefusesWrites {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("output stream refused the write"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("output stream refused the flush"))
    }
}

#[test]
fn the_stdio_session_serves_until_end_of_input() {
    let opts = opts_with(memory(&[]), &[]);
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        mcp_stdio_serve(
            &crate::sigpipe::Serving::for_test(),
            &opts,
            std::io::Cursor::new(Vec::<u8>::new()),
            &mut out
        ),
        ExitCode::SUCCESS
    );
    assert!(out.is_empty(), "no request means no response");
}

#[test]
fn a_stdio_session_stops_with_one_when_its_output_stream_refuses() {
    let opts = opts_with(memory(&[]), &[]);
    let frame = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{}}}"#;
    let input = std::io::Cursor::new(format!("{frame}\n").into_bytes());
    assert_eq!(
        mcp_stdio_serve(
            &crate::sigpipe::Serving::for_test(),
            &opts,
            input,
            &mut RefusesWrites
        ),
        ExitCode::from(1)
    );
}

#[test]
fn a_stdio_session_that_cannot_open_its_target_exits_1() {
    let bad = opts_with(args_of(&["--backend", "mysql"]), &[]);
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        mcp_stdio_serve(
            &crate::sigpipe::Serving::for_test(),
            &bad,
            std::io::Cursor::new(Vec::<u8>::new()),
            &mut out
        ),
        ExitCode::from(1)
    );
}
