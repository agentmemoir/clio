// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the `clio setup` command surface: the
//! interactive gating, the clean quit and back paths, and the help output.
//!
//! ## Owns
//! - Command-surface behavior under test for `setup_cli`
//!
//! ## Does not own
//! - The flag grammar (see `setup_flags_tests`)
//! - Usage/failure exit-code tests (see `setup_cli_fail_tests`)
//! - The apply path itself (see `setup_wizard_apply_tests`)
//!
//! ## Boundary
//!
//! Changes to setup's prompting rules should normally update tests here. Changes
//! to a question should not.
//!
//! Keep this module focused on the setup command surface.

use super::*;
use crate::setup_support_tests::{StubDocker, invoke_setup};

/// One `clio setup` run over injected streams and a Docker stub.
///
/// `--config` is added unless the caller supplied one, pointing at a path under
/// a directory that does not exist. Without it every test would read whatever
/// `clio setup` last wrote on the machine running the suite, and a test would
/// pass or fail depending on which backend that machine happened to use.
fn invoke(args: &[&str], input: &str, is_tty: bool) -> (ExitCode, String, String) {
    let mut full: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    if !full.iter().any(|arg| arg == "--config") {
        full.push("--config".to_owned());
        full.push(crate::setup_preview_fixtures::ABSENT_CONFIG.to_owned());
    }
    let config = PathBuf::from(crate::setup_preview_fixtures::ABSENT_CONFIG);
    let run = invoke_setup(
        &full,
        input,
        is_tty,
        &config,
        &StubDocker::ok(),
        &crate::setup_test_seam::Probes::ok(),
    );
    (run.code, run.out, run.err)
}

#[test]
fn yes_and_non_interactive_force_prompting_off_even_with_a_terminal() {
    for flag in ["--yes", "--non-interactive"] {
        let dir = crate::setup_support_tests::temp_dir("cli-mode");
        let config = dir.join("deployment.json");
        let run = invoke_setup(
            &[
                flag.to_owned(),
                "--database".to_owned(),
                "sqlite".to_owned(),
                "--embed".to_owned(),
                "online".to_owned(),
                "--embed-url".to_owned(),
                "https://127.0.0.1:1".to_owned(),
                "--embed-model".to_owned(),
                "m".to_owned(),
                "--embed-key".to_owned(),
                "k".to_owned(),
                "--rerank".to_owned(),
                "skip".to_owned(),
                "--extract".to_owned(),
                "skip".to_owned(),
                "--allow-untested".to_owned(),
                "--config".to_owned(),
                config.to_string_lossy().into_owned(),
            ],
            "never read\n",
            true,
            &config,
            &StubDocker::ok(),
            &crate::setup_test_seam::Probes::ok(),
        );
        assert_eq!(run.code, ExitCode::SUCCESS, "for {flag}: {}", run.err);
        // The run never prompted, so it never drew a question.
        assert!(
            !run.out.contains("Where should your memories live?"),
            "{}",
            run.out
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn leaving_at_the_first_question_exits_cleanly_with_no_message() {
    // The first question is the key store, and its two stores are options 1 and 2,
    // so the `Quit` item is 3. It is an item rather than a key because the prompt
    // reads raw keystrokes: a typed `q` would be swallowed by the prompt library
    // and reach no code at all.
    let dir = crate::setup_support_tests::temp_dir("cli-quit");
    let config = dir.join("deployment.json");
    let (code, out, err) = invoke(&["--config", &config.to_string_lossy()], "3\n", true);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("Nothing has been started"), "{out}");
    assert!(
        err.is_empty(),
        "a clean exit must print nothing on stderr: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_first_question_offers_quit_rather_than_going_back() {
    // There is nothing behind the first question, so the way out says so instead of
    // promising a return that cannot happen. The first question is the key store:
    // two stores, so the way out is 3.
    //
    // It is chosen rather than left unanswered, because a run-out-of-input is a
    // usage stop that prints on stderr, and this test is about what the menu says
    // on stdout. Choosing `Quit` also ends the run there, so the whole transcript is
    // the one question.
    let dir = crate::setup_support_tests::temp_dir("cli-first-back-out");
    let config = dir.join("deployment.json");
    let (code, out, err) = invoke(&["--config", &config.to_string_lossy()], "3\n", true);
    assert_eq!(code, ExitCode::SUCCESS, "err: {err}");
    assert!(out.contains("[3] Quit"), "{out}");
    assert!(!out.contains("[3] Go back"), "{out}");
    assert!(err.is_empty(), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_later_question_offers_going_back() {
    // Answer the key store, then look at the database question: the way out is
    // `Go back` there, because there is now something behind the operator.
    let dir = crate::setup_support_tests::temp_dir("cli-later-back-out");
    let config = dir.join("deployment.json");
    let (code, out, _err) = invoke(&["--config", &config.to_string_lossy()], "\n", true);
    assert_ne!(code, ExitCode::SUCCESS, "the run stops for want of input");
    let database_at = out
        .find("Where should your memories live?")
        .expect("the database question was asked");
    let tail = &out[database_at..];
    assert!(tail.contains("[4] Go back"), "{out}");
    assert!(!tail.contains("[4] Quit"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn help_prints_and_exits_zero_for_both_spellings() {
    for spelling in ["--help", "help"] {
        let (code, out, _err) = invoke(&[spelling], "", false);
        assert_eq!(code, ExitCode::SUCCESS, "for {spelling}");
        assert!(out.contains("clio setup [--database"), "{out}");
    }
}

#[test]
fn help_says_the_type_flag_is_gone() {
    let (_, out, _err) = invoke(&["--help"], "", false);
    // Naming the removed flag at all is what the help-text rule forbids, so the
    // sentence keeps the fact and drops the name.
    assert!(!out.contains("--type"), "{out}");
    assert!(
        out.contains("replaced an earlier single type selector"),
        "{out}"
    );
}
