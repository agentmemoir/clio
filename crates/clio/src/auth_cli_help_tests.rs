// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio auth token` help and usage text, and the refusal
//! of arguments a verb cannot honour.
//!
//! ## Owns
//! - Help and usage tests for the `auth` CLI surface
//! - Refusal tests for arguments a verb cannot honour
//!
//! ## Does not own
//! - The command implementation under test (`auth_cli`)
//! - Verb behavior tests (they live in `auth_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{add_args, records_on_disk, run_auth, scratch, tokens};
use super::*;
use std::fs;

#[test]
fn add_help_refuses_only_unusable_store_flags() {
    let store = scratch("add-help-flags");
    let (code, out, err) = run_auth(&store, &["token", "add", "--help"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("--bank BANK [--bank BANK]"), "{out}");
    assert!(
        out.contains("--db and --backend are refused because they cannot change what is written"),
        "{out}"
    );
    assert!(!out.contains("--bank are refused"), "{out}");
}

#[test]
fn help_and_unknown_input_follow_the_cli_convention() {
    let store = scratch("help");
    let (code, out, _) = run_auth(&store, &["--help"]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("clio auth token"), "{out}");

    let (code, out, _) = run_auth(&store, &["token", "--help"]);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(
        out.contains("add") && out.contains("list") && out.contains("revoke"),
        "{out}"
    );

    for verb in ["add", "list", "revoke"] {
        let (code, out, _) = run_auth(&store, &["token", verb, "--help"]);
        assert_eq!(code, ExitCode::SUCCESS, "{verb}");
        let expected = format!("usage: clio auth token {verb}");
        assert!(out.starts_with(&expected), "{out}");
    }

    let (code, _, err) = run_auth(&store, &["bogus"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    let (code, _, err) = run_auth(&store, &["token", "bogus"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    let (code, _, err) = run_auth(&store, &[]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    let (code, _, err) = run_auth(&store, &["token"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("requires a verb"), "{err}");
    let _ = fs::remove_file(&store);
}

#[test]
fn an_extra_positional_is_refused_by_every_verb() {
    // `add` and `revoke` already refused one; `list` did not, and the whole
    // point of the check is that an argument a verb cannot use is refused
    // rather than ignored.
    let store = scratch("extra-positional");
    let (code, _, _) = run_auth(&store, &add_args("ci", &["demo"]));
    assert_eq!(code, ExitCode::SUCCESS);
    for args in [
        &["token", "list", "extra"][..],
        &[
            "token", "add", "--label", "a", "--actor", "agent", "--bank", "b", "extra",
        ][..],
        &["token", "revoke", "--label", "ci", "extra"][..],
    ] {
        let (code, _, err) = run_auth(&store, args);
        assert_eq!(code, ExitCode::from(2), "{args:?}: {err}");
        assert!(err.contains("unexpected extra argument"), "{args:?}: {err}");
    }
    // The store is untouched by a refused argument.
    assert_eq!(records_on_disk(&store).len(), 1);
    let _ = fs::remove_file(&store);
}

#[test]
fn the_store_flags_a_verb_cannot_honour_are_refused() {
    // `FlagSpec::new` unions the globals into every spec, so without an
    // explicit refusal each of these parses and then does nothing. The sibling
    // archive surface refuses the same class with the same shape.
    let store = scratch("store-flags");
    let (code, _, _) = run_auth(&store, &add_args("ci", &["demo"]));
    assert_eq!(code, ExitCode::SUCCESS);

    let cases: Vec<Vec<&str>> = vec![
        vec!["token", "list", "--db", "/nonexistent.sqlite"],
        vec!["token", "list", "--backend", "postgres"],
        vec!["token", "list", "--actor", "agent"],
        vec!["token", "list", "--bank", "demo"],
        vec![
            "token",
            "revoke",
            "--label",
            "ci",
            "--db",
            "/nonexistent.sqlite",
        ],
        vec!["token", "revoke", "--label", "ci", "--bank", "demo"],
        vec![
            "token", "add", "--label", "x", "--actor", "agent", "--bank", "demo", "--db", "/x",
        ],
    ];
    for args in cases {
        let (code, _, err) = run_auth(&store, &args);
        assert_eq!(code, ExitCode::from(2), "{args:?}: {err}");
        assert!(
            err.contains("cannot change what it does"),
            "{args:?}: {err}"
        );
    }
    // The record `add` wrote before the refusals is still the only one, so no
    // refusal wrote anything.
    assert_eq!(records_on_disk(&store).len(), 1);
    // The flags `add` does read are still accepted there.
    let (code, _, err) = run_auth(&store, &add_args("two", &["demo", "other"]));
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let _ = fs::remove_file(&store);
}

#[test]
fn repeated_flag_parsing_keeps_every_occurrence() {
    // The parser contract the auth verbs rely on, pinned at the parse layer.
    let repeated_spec = spec("auth token add");
    let parsed = cli_args::parse(
        &repeated_spec,
        tokens(&["--bank", "a", "--bank", "b", "--label", "l"]),
    )
    .expect("parse");
    assert_eq!(parsed.values_of("bank"), &["a".to_owned(), "b".to_owned()]);
    assert_eq!(parsed.value("label"), Some("l"));
    // A repeated non-repeatable flag is refused, not last-wins.
    let plain_spec = spec("auth token revoke");
    let error = cli_args::parse(&plain_spec, tokens(&["--label", "x", "--label", "y"]))
        .expect_err("repeated --label");
    assert!(
        error.message().contains("--label was given more than once"),
        "{}",
        error.message()
    );
}
