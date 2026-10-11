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
