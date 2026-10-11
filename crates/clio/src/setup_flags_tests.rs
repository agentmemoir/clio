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
//! - The setup flag metadata tests: the allowed-value lists and value kinds
//!   the record documents, checked against the parsers that enforce them
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

/// The setup record's accepted flags: the table the parser reads, so a test
/// asks the record rather than a second list.
fn accepted() -> &'static [crate::cli_command_spec::Flag] {
    crate::cli_command_registry::find("setup")
        .expect("`setup` is registered")
        .documented
}

/// Every flag the setup record documents carries complete metadata: a
/// description, and for a value flag a placeholder and a non-empty kind.
#[test]
fn every_accepted_setup_flag_carries_complete_metadata() {
    for flag in accepted() {
        if let Err(error) = crate::cli_command_registry::check_flag_metadata(flag) {
            panic!("{error}");
        }
    }
}

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
    // The parser and the record are written from the same table, so a flag can
    // only get into one of them by hand. This asks the parser directly for each
    // name, which is the failure the wizard would otherwise show as "unexpected
    // argument" on a flag the record accepts.
    for flag in accepted().iter().filter(|flag| flag.value.is_some()) {
        let raw = vec![format!("--{}", flag.long), "value".to_owned()];
        let parsed = parse_setup_args(&raw);
        assert!(
            parsed.is_ok(),
            "--{} is in the accepted table but the parser refuses it: {parsed:?}",
            flag.long
        );
        assert_eq!(
            parsed.expect("a value").get(flag.long).map(String::as_str),
            Some("value"),
            "--{} did not keep its value",
            flag.long
        );
    }
}

#[test]
fn help_documents_every_value_flag_it_accepts() {
    let mut out: Vec<u8> = Vec::new();
    print_help(&mut out);
    let text = String::from_utf8(out).expect("utf8");
    for flag in accepted() {
        assert!(
            text.contains(flag.long),
            "--help does not document {}",
            flag.long
        );
    }
    assert!(text.contains("--allow-untested"), "{text}");
    assert!(text.contains("--help"), "{text}");
}

#[test]
fn the_retired_rotate_flag_names_why_it_is_gone() {
    let error =
        parse_setup_args(&["--rotate-database-password".to_owned()]).expect_err("a retired flag");
    assert_eq!(error, ROTATE_REMOVED);
    assert!(error.contains("no password"), "{error}");
}

#[test]
fn help_is_accepted_by_the_parser_itself() {
    // `--help` is answered before the parser runs on the real surface, but the
    // parser still accepts it by name so a valued flag may follow it; a test
    // pins that here rather than trusting the surface ordering.
    let flags = parse_setup_args(&["--help".to_owned()]).expect("parse");
    assert_eq!(flags.get("help").map(String::as_str), Some("true"));
}

#[test]
fn a_blank_value_is_refused_by_name() {
    for value in [String::new(), "  ".to_owned()] {
        let error = parse_setup_args(&["--config".to_owned(), value]).expect_err("a blank value");
        assert!(
            error.contains("--config must not be empty or whitespace-only"),
            "{error}"
        );
    }
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
            "database-ssl-rootcert",
            "setup_flows_db_typed.rs",
            "flag(\"database-ssl-rootcert\")",
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
    for flag in accepted().iter().filter(|flag| flag.value.is_some()) {
        let (_, file, needle) = READERS
            .iter()
            .find(|(name, _, _)| *name == flag.long)
            .unwrap_or_else(|| panic!("no reader recorded for --{}", flag.long));
        let text = std::fs::read_to_string(dir.join(file)).expect("a source file");
        assert!(
            text.contains(needle),
            "--{} is recorded as read in {file} but `{needle}` is gone",
            flag.long
        );
    }
    assert_eq!(
        READERS.len(),
        accepted()
            .iter()
            .filter(|flag| flag.value.is_some())
            .count(),
        "a recorded reader names a flag that no longer exists"
    );
}

/// The allowed-value list one choice flag documents.
///
/// A non-choice flag here is a test bug, not a refusal: the callers below only
/// ask for flags they know are choices.
fn allowed(flag: &str) -> &'static [&'static str] {
    let documented = accepted()
        .iter()
        .find(|documented| documented.long == flag)
        .unwrap_or_else(|| panic!("--{flag} is not in the accepted table"));
    let value = documented
        .value
        .unwrap_or_else(|| panic!("--{flag} takes no value"));
    match value.kind {
        crate::cli_command_spec::ValueKind::Enum(allowed) => allowed,
        other => panic!("--{flag} is not a choice: {other:?}"),
    }
}

/// Every spelling an enum flag documents is one its validator accepts, so the
/// record never promises a value the wizard would refuse.
#[test]
fn every_documented_enum_value_is_accepted_by_its_validator() {
    for value in allowed("database") {
        assert!(
            crate::setup_plan_db::DatabaseChoice::parse(value).is_some(),
            "--database documents `{value}` but its parser refuses it"
        );
    }
    for value in allowed("database-connect") {
        assert!(
            crate::setup_plan_db::HandOver::parse(value).is_some(),
            "--database-connect documents `{value}` but its parser refuses it"
        );
    }
    for value in allowed("embed").iter().chain(allowed("rerank")) {
        assert!(
            crate::setup_services::ServiceRole::parse(value).is_some(),
            "the service-role parser refuses `{value}`"
        );
    }
}

/// `--extract`'s validator is an inline match in its own file, so the
/// spellings are pinned by the same source-needle pattern the readers table
/// uses: the whole skip arm, plus each documented value named in the file.
#[test]
fn the_extract_enum_names_only_spellings_its_validator_accepts() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/setup_step_extract.rs"),
    )
    .expect("a source file");
    assert!(
        text.contains("\"skip\" | \"none\" | \"online\" | \"hosted\""),
        "the extract validator no longer accepts the skip spellings: {text}"
    );
    for value in allowed("extract") {
        assert!(
            text.contains(&format!("\"{value}\"")),
            "--extract documents `{value}` but the validator never names it"
        );
    }
}

/// `--database-port` takes text, not a uint: its validator accepts a
/// comma-separated port list, so a whole-number kind would promise a shape the
/// wizard refuses.
#[test]
fn the_port_flag_takes_text_so_a_list_fits_its_validator() {
    let documented = accepted()
        .iter()
        .find(|documented| documented.long == "database-port")
        .expect("--database-port is in the accepted table");
    let kind = documented.value.expect("a value flag").kind;
    assert_eq!(kind, crate::cli_command_spec::ValueKind::Text, "{kind:?}");
}
