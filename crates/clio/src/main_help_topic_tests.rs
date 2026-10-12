// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how the `help` branch of the CLI dispatch routes a topic:
//! the re-dispatch onto the named command and the unknown-topic error path.
//!
//! ## Owns
//! - The `clio help <topic>` re-dispatch routing and exit code
//! - The `clio help <unknown topic>` usage-error exit code
//!
//! ## Does not own
//! - The topic parser under test (`cli_top_level`)
//! - The command help screens the re-dispatch enters
//! - The plain `clio help` and `--json` catalog tests (`main_tests`)
//!
//! ## Boundary
//!
//! Tests here break when help-topic routing or its exit codes change; they
//! never change it.
//!
//! Keep this module focused on help-topic routing.

use super::*;

#[test]
fn help_topic_redispatches_to_the_command_help() {
    // `clio help mcp` must resolve to the `mcp` verb with `--help` appended,
    // which the dispatch re-enters as `clio mcp --help`. A regression that
    // drops the topic prints the plain catalog yet still exits SUCCESS, so
    // the exit code alone cannot catch it: the routing decision below is the
    // assertion that fails when the topic is ignored.
    let (verb, kept) = crate::cli_top_level::help_topic(&["mcp".into()])
        .expect("a known topic resolves")
        .expect("a topic was named");
    assert_eq!(verb, "mcp");
    assert_eq!(kept, vec!["--help".to_owned()]);
    // Both ends of the re-dispatch succeed: the topic route and the target
    // it re-enters.
    assert_eq!(run(vec!["help".into(), "mcp".into()]), ExitCode::SUCCESS);
    assert_eq!(run(vec!["mcp".into(), "--help".into()]), ExitCode::SUCCESS);
}

#[test]
fn help_unknown_topic_exits_two() {
    // An unknown topic is an error, never a silent full catalog.
    assert_eq!(
        run(vec!["help".into(), "frobnicate".into()]),
        ExitCode::from(2)
    );
}

#[test]
fn help_reaches_the_catalog_with_a_global_value_flag() {
    // The value of a global value flag is not a topic, whichever side of the
    // verb the global sits on, and a topic after it still resolves.
    for argv in [
        vec!["help".into(), "--db".into(), "/tmp/led.db".into()],
        vec!["--db".into(), "/tmp/led.db".into(), "help".into()],
        vec![
            "help".into(),
            "--db".into(),
            "/tmp/led.db".into(),
            "backup".into(),
        ],
    ] {
        assert_eq!(run(argv.clone()), ExitCode::SUCCESS, "{argv:?}");
    }
}

#[test]
fn the_json_catalog_is_selected_when_a_global_value_flag_is_present() {
    // `--json` is chosen before the topic, so a global value flag cannot turn
    // the catalog request into an unknown topic. The two units below are the
    // pair the help branch runs in that order.
    let argv = [
        "--db".to_owned(),
        "/tmp/led.db".to_owned(),
        "--json".to_owned(),
    ];
    assert!(crate::cli_top_level::wants_json_catalog(&argv));
    assert_eq!(
        crate::cli_top_level::help_topic(&argv).expect("no error"),
        None
    );
    assert_eq!(
        run(vec![
            "help".into(),
            "--db".into(),
            "/tmp/led.db".into(),
            "--json".into(),
        ]),
        ExitCode::SUCCESS
    );
}

/// `clio help --help` and `clio help -h` are word-less help requests, so they
/// print the top-level document exactly as `clio help` does. Every other leaf
/// command prints its registry record's usage for `--help`; `help` is the
/// exception by design, because the document is its own subject. This pins the
/// decision so a later consistency pass cannot "fix" the flag into the record's
/// usage line while the `help help` topic path would still print the document.
#[test]
fn help_with_a_help_flag_prints_the_top_level_document() {
    for argv in [
        vec!["help".to_owned(), "--help".to_owned()],
        vec!["help".to_owned(), "-h".to_owned()],
    ] {
        assert_eq!(
            crate::cli_top_level::help_topic(&argv[1..]).expect("no error"),
            None,
            "a help flag is not a topic: {argv:?}"
        );
        assert_eq!(run(argv.clone()), ExitCode::SUCCESS, "{argv:?}");
    }
}
