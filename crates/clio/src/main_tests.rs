// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` CLI surface: command dispatch, exit codes, the
//! database-precedence option builder, and help/credential masking.
//!
//! ## Owns
//! - Dispatch and exit-code tests for `version`, `health`, `help`, `mcp`
//! - MCP option resolution tests over an injected environment lookup
//! - Help-output and credential-masking tests
//!
//! ## Does not own
//! - The CLI implementation under test (`main`)
//! - The `clio ops` surface tests (`ops_cli`)
//! - End-to-end process tests over a real binary (`tests/`)
//!
//! ## Boundary
//!
//! Tests here break when the CLI command surface or exit-code contract changes;
//! they never change it.
//!
//! Keep this module focused on the binary's command surface.

use super::mcp_cli::*;
use super::*;
use crate::cli_flags_test_support::flags_of;

#[test]
fn default_args_print_version() {
    assert_eq!(run(Vec::<String>::new()), ExitCode::SUCCESS);
}

#[test]
fn version_aliases() {
    for cmd in ["version", "--version", "-V"] {
        assert_eq!(run(vec![cmd.into()]), ExitCode::SUCCESS);
    }
}

#[test]
fn health_ok() {
    assert_eq!(run(vec!["health".into()]), ExitCode::SUCCESS);
}

#[test]
fn help_aliases() {
    for cmd in ["help", "--help", "-h"] {
        assert_eq!(run(vec![cmd.into()]), ExitCode::SUCCESS);
    }
}

#[test]
fn setup_help_exits_zero() {
    assert_eq!(
        run(vec!["setup".into(), "--help".into()]),
        ExitCode::SUCCESS
    );
}

#[test]
fn setup_usage_errors_exit_2() {
    assert_eq!(
        run(vec!["setup".into(), "--bogus".into()]),
        ExitCode::from(2)
    );
}

#[test]
fn unknown_command_exits_2() {
    assert_eq!(run(vec!["nope".into()]), ExitCode::from(2));
}

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
        clio_mcp::runtime::McpState::open(
            &mcp_options(["--db", "sqlite::memory:"].map(str::to_owned)).expect("options"),
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

#[test]
fn parse_flags_pairs_and_bare_flags() {
    let flags = flags_of(&["--db", "x", "--sessions"]);
    assert_eq!(flags.get("db").map(String::as_str), Some("x"));
    assert_eq!(flags.get("sessions").map(String::as_str), Some("true"));
    assert!(!flags.contains_key("missing"));
}

#[test]
fn parse_flags_bare_flag_before_another_flag() {
    // `--bind` has no value; `--db` must not be consumed as that value.
    let flags = flags_of(&["--bind", "--db", "sqlite::memory:", "--sessions"]);
    assert_eq!(flags.get("bind").map(String::as_str), Some("true"));
    assert_eq!(flags.get("db").map(String::as_str), Some("sqlite::memory:"));
    assert_eq!(flags.get("sessions").map(String::as_str), Some("true"));
}

#[test]
fn parse_flags_equals_syntax() {
    // `--flag=value` carries its own value; the value may itself contain `=`.
    let flags = flags_of(&[
        "--bind=127.0.0.1:34305",
        "--db=sqlite::memory:",
        "--sessions",
    ]);
    assert_eq!(
        flags.get("bind").map(String::as_str),
        Some("127.0.0.1:34305")
    );
    assert_eq!(flags.get("db").map(String::as_str), Some("sqlite::memory:"));
    assert_eq!(flags.get("sessions").map(String::as_str), Some("true"));
}

#[test]
fn parse_flags_equals_value_beats_following_flag() {
    // `--bind=X --db Y`: the token after `--bind=X` must not be swallowed.
    let flags = flags_of(&["--bind=X", "--db", "y"]);
    assert_eq!(flags.get("bind").map(String::as_str), Some("X"));
    assert_eq!(flags.get("db").map(String::as_str), Some("y"));
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
fn database_help_block_masks_credentials() {
    let block = crate::cli_top_level::database_help_block(&|key: &str| match key {
        "CLIO_DATABASE_URL" => Some("postgres://user:s3cret@127.0.0.1:5432/am".into()),
        _ => None,
    });
    assert!(block.contains("postgres://***@127.0.0.1:5432/am"));
    assert!(!block.contains("s3cret"));
    assert!(block.contains("precedence:"));
}

/// The precedence line must name the variable the resolver reads today and must
/// not still offer the old spelling. Shape, not the sentence, because the
/// sentence will be worded again.
#[test]
fn the_precedence_line_names_only_the_prefixed_database_variable() {
    let block = crate::cli_top_level::database_help_block(&|_: &str| None);
    let line = block
        .lines()
        .find(|l| l.trim_start().starts_with("precedence:"))
        .expect("the block states a precedence order");
    let without_prefixed = line.replace("CLIO_DATABASE_URL", "");
    assert!(line.contains("CLIO_DATABASE_URL"), "{line}");
    assert!(!without_prefixed.contains("DATABASE_URL"), "{line}");
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

#[test]
fn globals_before_the_verb_are_routed() {
    // `--db` before the verb must still reach the read command.
    assert_eq!(
        run(vec![
            "--db".into(),
            "sqlite::memory:".into(),
            "recall".into(),
            "q".into()
        ]),
        ExitCode::SUCCESS
    );
}

#[test]
fn unknown_command_typo_exits_two() {
    assert_eq!(run(vec!["recal".into()]), ExitCode::from(2));
}

#[test]
fn help_json_exits_zero() {
    assert_eq!(run(vec!["help".into(), "--json".into()]), ExitCode::SUCCESS);
}

#[test]
fn read_command_unknown_flag_exits_two() {
    assert_eq!(
        run(vec![
            "recall".into(),
            "q".into(),
            "--limt".into(),
            "2".into(),
            "--db".into(),
            "sqlite::memory:".into()
        ]),
        ExitCode::from(2)
    );
}

#[test]
fn read_command_opens_and_reads_in_memory() {
    assert_eq!(
        run(vec![
            "stats".into(),
            "--db".into(),
            "sqlite::memory:".into(),
            "--output".into(),
            "json".into()
        ]),
        ExitCode::SUCCESS
    );
}
