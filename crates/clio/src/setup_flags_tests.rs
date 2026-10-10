// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the `clio setup` flag grammar: the
//! flag sets, the repeatable flag, the `--type` retirement, and the help text.
//!
//! ## Owns
//! - Flag parsing behavior under test for `setup_flags`
//!
//! ## Does not own
//! - What the flags mean (see `setup_cli_tests` and the flow suites)
//! - Usage exit codes (see `setup_cli_fail_tests`)
//!
//! ## Boundary
//!
//! Changes to a flag's name, arity, or help text should normally update tests
//! here. Changes to a question should not.
//!
//! Keep this module focused on the command line's shape.

use super::*;

#[test]
fn a_boolean_flag_may_precede_a_valued_one() {
    let flags = parse_setup_args(&[
        "--yes".to_owned(),
        "--database".to_owned(),
        "sqlite".to_owned(),
    ])
    .expect("parse");
    assert_eq!(flags.get("yes").map(String::as_str), Some("true"));
    assert_eq!(flags.get("database").map(String::as_str), Some("sqlite"));
}

#[test]
fn a_repeatable_flag_accumulates_rather_than_replacing() {
    let flags = parse_setup_args(&[
        "--database-option".to_owned(),
        "options=-c%20k1".to_owned(),
        "--database-option".to_owned(),
        "application_name=clio".to_owned(),
    ])
    .expect("parse");
    assert_eq!(
        flags.get("database-option").map(String::as_str),
        Some("options=-c%20k1\napplication_name=clio")
    );
}

#[test]
fn a_non_repeatable_value_flag_given_twice_is_refused_by_name() {
    let error = parse_setup_args(&[
        "--config".to_owned(),
        "a.json".to_owned(),
        "--config".to_owned(),
        "b.json".to_owned(),
    ])
    .expect_err("repeated --config");
    assert!(
        error.contains("--config was given more than once"),
        "{error}"
    );
}

#[test]
fn an_unknown_flag_positional_or_missing_value_is_a_refusal() {
    assert!(parse_setup_args(&["--nope".to_owned()]).is_err());
    assert!(parse_setup_args(&["stray".to_owned()]).is_err());
    assert!(parse_setup_args(&["--database".to_owned()]).is_err());
    assert!(
        parse_setup_args(&["--database".to_owned(), "--yes".to_owned()]).is_err(),
        "a following flag is not a value"
    );
}

#[test]
fn the_retired_type_flag_names_its_replacement() {
    let error =
        parse_setup_args(&["--type".to_owned(), "custom".to_owned()]).expect_err("a retired flag");
    assert_eq!(error, TYPE_REMOVED);
    assert!(error.contains("--database"), "{error}");
    assert!(error.contains("--embed"), "{error}");
}

#[test]
fn every_value_flag_the_parser_accepts_is_accepted_by_name() {
    // The parser and the help text are written from the same list, so a flag can
    // only get into one of them by hand. This asks the parser directly for each
    // name, which is the failure the wizard would otherwise show as "unexpected
    // argument" on a flag the help screen prints.
    for flag in VALUE_FLAGS {
        let raw = vec![format!("--{flag}"), "value".to_owned()];
        let parsed = parse_setup_args(&raw);
        assert!(
            parsed.is_ok(),
            "--{flag} is in the value-flag list but the parser refuses it: {parsed:?}"
        );
        assert_eq!(
            parsed.expect("a value").get(*flag).map(String::as_str),
            Some("value"),
            "--{flag} did not keep its value"
        );
    }
}

#[test]
fn help_documents_every_value_flag_it_accepts() {
    let mut out: Vec<u8> = Vec::new();
    print_help(&mut out);
    let text = String::from_utf8(out).expect("utf8");
    for flag in BOOLEAN_FLAGS.iter().chain(VALUE_FLAGS) {
        assert!(text.contains(flag), "--help does not document {flag}");
    }
    assert!(text.contains("--allow-untested"), "{text}");
}

#[test]
fn every_value_flag_is_read_by_the_wizard() {
    // An accepted-but-ignored flag is the bug class that dropped the provider
    // flags: the grammar and the help text promised them while no code read
    // them. Each row names the file that reads the flag and a distinctive
    // source text; a new flag without a row fails here until its reader is
    // recorded, and a reader that is deleted fails until the row is.
    const READERS: &[(&str, &str, &str)] = &[
        ("config", "setup_cli.rs", "get(\"config\")"),
        ("compose-dir", "setup_cli.rs", "get(\"compose-dir\")"),
        ("database", "setup_flows_db.rs", "flag(\"database\")"),
        ("db-path", "setup_flows_db.rs", "flag(\"db-path\")"),
        (
            "database-url",
            "setup_flows_db_paste.rs",
            "flag(\"database-url\")",
        ),
        (
            "database-connect",
            "setup_flows_db_paste.rs",
            "flag(\"database-connect\")",
        ),
        (
            "database-host",
            "setup_flows_db_typed.rs",
            "\"database-host\"",
        ),
        (
            "database-port",
            "setup_flows_db_typed.rs",
            "\"database-port\"",
        ),
        (
            "database-name",
            "setup_flows_db_typed.rs",
            "\"database-name\"",
        ),
        (
            "database-user",
            "setup_flows_db_typed.rs",
            "\"database-user\"",
        ),
        (
            "database-password",
            "setup_flows_db_typed.rs",
            "flag(\"database-password\")",
        ),
        (
            "database-ssl",
            "setup_flows_db.rs",
            "flag(\"database-ssl\")",
        ),
        (
            "database-option",
            "setup_flows_db_typed.rs",
            "flag_values(\"database-option\")",
        ),
        ("embed", "setup_cli.rs", "flag(\"embed\")"),
        ("embed-url", "setup_step_ask.rs", "{name}-url"),
        ("embed-model", "setup_step_ask.rs", "{name}-model"),
        ("embed-key", "setup_step_ask.rs", "{name}-key"),
        ("embed-provider", "setup_step_ask.rs", "{name}-provider"),
        ("key-store", "setup_key_source.rs", "flag(FLAG)"),
        ("sync-store", "setup_sync_key_source.rs", "flag(FLAG)"),
        ("embed-dims", "setup_cli.rs", "flag(\"embed-dims\")"),
        ("rerank", "setup_cli.rs", "flag(\"rerank\")"),
        ("rerank-url", "setup_step_ask.rs", "{name}-url"),
        ("rerank-model", "setup_step_ask.rs", "{name}-model"),
        ("rerank-key", "setup_step_ask.rs", "{name}-key"),
        ("rerank-provider", "setup_step_ask.rs", "{name}-provider"),
        ("extract", "setup_step_extract.rs", "flag(\"extract\")"),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for flag in VALUE_FLAGS {
        let (_, file, needle) = READERS
            .iter()
            .find(|(name, _, _)| name == flag)
            .unwrap_or_else(|| panic!("no reader recorded for --{flag}"));
        let text = std::fs::read_to_string(dir.join(file)).expect("a source file");
        assert!(
            text.contains(needle),
            "--{flag} is recorded as read in {file} but `{needle}` is gone"
        );
    }
    assert_eq!(
        READERS.len(),
        VALUE_FLAGS.len(),
        "a recorded reader names a flag that no longer exists"
    );
}
