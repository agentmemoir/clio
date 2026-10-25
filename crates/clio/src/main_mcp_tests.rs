// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio mcp` command surface: subcommand dispatch and
//! exit codes, schema export, the stdio and http transports, and option
//! resolution over an injected environment lookup.
//!
//! ## Owns
//! - Dispatch and exit-code tests for `mcp` and its subcommands
//! - `mcp` option resolution tests over an injected environment lookup
//! - Schema-export, stdio, and http bind configuration tests
//!
//! ## Does not own
//! - The MCP command implementation under test (`mcp_cli`)
//! - The core command dispatch tests (`main_tests`)
//! - The MCP protocol and server internals (`clio_mcp`)
//!
//! ## Boundary
//!
//! Tests here break when the `mcp` command surface, its exit codes, or its
//! option precedence changes; they never change it.
//!
//! Keep this module focused on the `mcp` command surface.

use super::mcp_cli::*;
use super::*;
use crate::cli_flags_test_support::flags_of;

#[test]
fn mcp_unknown_subcommand_exits_2() {
    assert_eq!(run(vec!["mcp".into(), "bogus".into()]), ExitCode::from(2));
}

#[test]
fn mcp_schema_export_writes_file() {
    let path = std::env::temp_dir().join(format!(
        "am-schema-{}-{:?}.json",
        std::process::id(),
        std::thread::current().id()
    ));
    let code = run(vec![
        "mcp".into(),
        "schema-export".into(),
        "--out".into(),
        path.to_string_lossy().into_owned(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let raw = std::fs::read_to_string(&path).expect("written pack");
    let pack: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(pack["format"], "am_tool_schema_pack");
    assert_eq!(pack["mcp_protocol_revision"], "2025-11-25");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mcp_schema_export_accepts_path_alias() {
    let path = std::env::temp_dir().join(format!(
        "am-schema-path-{}-{:?}.json",
        std::process::id(),
        std::thread::current().id()
    ));
    let code = run(vec![
        "mcp".into(),
        "schema-export".into(),
        "--path".into(),
        path.to_string_lossy().into_owned(),
    ]);
    assert_eq!(code, ExitCode::SUCCESS);
    let raw = std::fs::read_to_string(&path).expect("written pack");
    let pack: serde_json::Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(pack["format"], "am_tool_schema_pack");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mcp_schema_export_stdout() {
    assert_eq!(
        run(vec!["mcp".into(), "schema-export".into()]),
        ExitCode::SUCCESS
    );
}

#[test]
fn mcp_stdio_serves_then_eof() {
    // Feeds an in-memory empty stream so the test never blocks on the
    // inherited process stdin (`run` would lock the real stdin). The
    // explicit DB keeps the test independent of the host environment.
    let handler = clio_mcp::protocol::McpHandler::new(
        clio_mcp::runtime::McpState::open_with_effective(
            &mcp_options(["--db", "sqlite::memory:"].map(str::to_owned)).expect("options"),
            &serde_json::json!({}),
        )
        .expect("state"),
    );
    let mut out = Vec::new();
    clio_mcp::stdio::serve(&handler, std::io::Cursor::new(Vec::new()), &mut out)
        .expect("serve until EOF");
}

#[test]
fn mcp_stdio_unknown_backend_exits_1() {
    assert_eq!(
        run(vec![
            "mcp".into(),
            "stdio".into(),
            "--backend".into(),
            "mysql".into()
        ]),
        ExitCode::from(1)
    );
}

#[test]
fn mcp_http_non_loopback_without_token_fails_closed() {
    assert_eq!(
        run(vec![
            "mcp".into(),
            "http".into(),
            "--bind".into(),
            "0.0.0.0:0".into(),
            "--db".into(),
            "sqlite::memory:".into()
        ]),
        ExitCode::from(1)
    );
}

#[test]
fn mcp_http_bad_bind_exits_1() {
    assert_eq!(
        run(vec![
            "mcp".into(),
            "http".into(),
            "--bind".into(),
            "999.1.1.1:1".into(),
            "--db".into(),
            "sqlite::memory:".into()
        ]),
        ExitCode::from(1)
    );
}

/// Build options with an injected lookup (no host-env dependence).
fn opts_with(args: &[&str], pairs: &[(&str, &str)]) -> clio_mcp::runtime::McpOpenOptions {
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    mcp_options_with(args, &|key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    })
    .expect("options resolve")
}

#[test]
fn mcp_options_flag_beats_env_url() {
    let opts = opts_with(
        &["--db", "./x.db"],
        &[("CLIO_DATABASE_URL", "postgres://h/db")],
    );
    assert_eq!(opts.database_url, "./x.db");
    assert_eq!(opts.backend, "sqlite");
}

#[test]
fn mcp_options_infers_postgres_from_env_url() {
    let opts = opts_with(&[], &[("CLIO_DATABASE_URL", "postgres://h/db")]);
    assert_eq!(opts.database_url, "postgres://h/db");
    assert_eq!(opts.backend, "postgres");
}

#[test]
fn mcp_options_explicit_backend_beats_inference() {
    let opts = opts_with(
        &["--backend", "sqlite"],
        &[("CLIO_DATABASE_URL", "postgres://h/db")],
    );
    assert_eq!(opts.backend, "sqlite");
}

#[test]
fn mcp_options_data_dir_default() {
    let opts = opts_with(&[], &[("CLIO_DATA_DIR", "/data")]);
    assert_eq!(opts.database_url, "/data/clio.db");
    assert_eq!(opts.backend, "sqlite");
}

#[test]
fn memory_value_stays_volatile() {
    let opts = opts_with(&["--db", "sqlite::memory:"], &[("CLIO_DATA_DIR", "/data")]);
    assert_eq!(opts.database_url, "sqlite::memory:");
}

#[test]
fn mcp_subcommand_help_exits_0() {
    assert_eq!(
        run(vec!["mcp".into(), "stdio".into(), "--help".into()]),
        ExitCode::SUCCESS
    );
    assert_eq!(
        run(vec!["mcp".into(), "http".into(), "-h".into()]),
        ExitCode::SUCCESS
    );
}

#[test]
fn mcp_stdio_serve_serves_and_reports_open_failure() {
    let ok = opts_with(&["--db", "sqlite::memory:"], &[]);
    let mut out = Vec::new();
    assert_eq!(
        mcp_stdio_serve(
            &crate::sigpipe::Serving::for_test(),
            &ok,
            std::io::Cursor::new(Vec::new()),
            &mut out
        ),
        ExitCode::SUCCESS
    );

    let bad = opts_with(&["--backend", "mysql"], &[]);
    let mut out = Vec::new();
    assert_eq!(
        mcp_stdio_serve(
            &crate::sigpipe::Serving::for_test(),
            &bad,
            std::io::Cursor::new(Vec::new()),
            &mut out
        ),
        ExitCode::from(1)
    );
}

#[test]
fn http_config_reads_flags() {
    let flags = flags_of(&["--bind", "127.0.0.1:9999", "--sessions"]);
    let cfg = http_config(&flags);
    // `bind` is filled in with the resolved address when serving, so the config
    // itself keeps the transport default until a listener is chosen.
    assert_eq!(cfg.bind, "127.0.0.1:0");
    assert!(cfg.sessions_enabled);

    let default = http_config(&flags_of(&[]));
    assert_eq!(default.bind, "127.0.0.1:0");
    assert!(!default.sessions_enabled);
    assert!(!default.insecure_bind_any, "off unless the flag says true");

    let opted_in = http_config(&flags_of(&["--insecure-bind-any"]));
    assert!(opted_in.insecure_bind_any, "the bare flag turns it on");
    let spelled_off = http_config(&flags_of(&["--insecure-bind-any=false"]));
    assert!(!spelled_off.insecure_bind_any, "a false value stays off");
}

#[test]
fn mcp_http_help_documents_auto_bind() {
    let help = mcp_http_help();
    assert!(help.contains("34300-34309"), "range missing: {help}");
    assert!(help.contains("bare `--bind`"), "auto rule missing: {help}");
    assert!(
        help.contains("HOST:PORT"),
        "explicit override missing: {help}"
    );
}

#[test]
fn mcp_schema_export_write_failure_exits_1() {
    let bad = std::env::temp_dir()
        .join(format!("am-missing-{}", std::process::id()))
        .join("out.json");
    assert_eq!(
        run(vec![
            "mcp".into(),
            "schema-export".into(),
            "--out".into(),
            bad.to_string_lossy().into_owned(),
        ]),
        ExitCode::from(1)
    );
}

#[test]
fn mcp_http_open_failure_exits_1() {
    assert_eq!(
        run(vec![
            "mcp".into(),
            "http".into(),
            "--backend".into(),
            "mysql".into()
        ]),
        ExitCode::from(1)
    );
}
