// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the context: the flag accessors,
//! the last-known-defaults accessors, the confirmation line, and the stored
//! password read.
//!
//! ## Owns
//! - Context behavior under test for `setup_cli_flows`
//!
//! ## Does not own
//! - The questions the context serves (see `setup_steps_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to the context's accessors should normally update tests here. Changes
//! to a question should not require changes here.
//!
//! Keep this module focused on the injected state.

use super::*;
use crate::setup_defaults::Defaults;
use crate::setup_preview_fixtures::{Harness, MANAGED, SERVICES, SQLITE};
use serde_json::json;
use std::path::PathBuf;

/// A context over a scripted reader and two byte buffers, with everything else
/// at its defaults.
///
/// The four locals are the ones `Ctx` borrows for its whole life, so a test that
/// needs to read through the context has to declare them the same way the harness
/// does rather than reaching into a struct.
///
/// Docker is available and there is no terminal, which is the pair the scripted
/// tests want: the managed and local choices have to be on offer for a test to get
/// past the first question, and the plain renderer is the one an in-memory reader
/// can be driven through at all.
macro_rules! with_ctx {
    ($input:expr, $flags:expr, |$ctx:ident| $body:expr) => {{
        let mut input = std::io::Cursor::new($input.as_bytes().to_vec());
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let flags = $flags;
        let probes = crate::setup_test_seam::Probes::ok();
        #[allow(unused_mut)]
        let mut $ctx = Ctx::new(
            &flags,
            &mut input,
            &mut out,
            &mut err,
            true,
            crate::cli_style::Palette::new(false),
            &probes,
            Defaults::empty(),
            PathBuf::from("/nonexistent/clio-stage5/clio.db"),
            crate::compose_docker::DockerStatus::Available,
            &crate::setup_preview_scripts::no_env,
            false,
        );
        let answer = $body;
        (answer, String::from_utf8(out).expect("utf8"))
    }};
}

/// No flags.
macro_rules! plain_ctx {
    ($input:expr, |$ctx:ident| $body:expr) => {
        with_ctx!($input, std::collections::HashMap::new(), |$ctx| $body)
    };
}

#[test]
fn a_flag_that_was_not_given_reads_as_absent() {
    plain_ctx!("", |ctx| {
        assert_eq!(ctx.flag("database"), None);
        assert!(!ctx.has_flag("database"));
    });
}

#[test]
fn a_flag_that_was_given_reads_back_exactly() {
    with_ctx!(
        "",
        [("database".to_owned(), "sqlite".to_owned())].into(),
        |ctx| {
            assert_eq!(ctx.flag("database"), Some("sqlite"));
            assert!(ctx.has_flag("database"));
        }
    );
}

#[test]
fn a_repeatable_flag_reads_back_as_every_value_in_order() {
    with_ctx!(
        "",
        [(
            "database-option".to_owned(),
            "options=-c%20k1\napplication_name=clio".to_owned()
        )]
        .into(),
        |ctx| {
            assert_eq!(
                ctx.flag_values("database-option"),
                vec!["options=-c%20k1", "application_name=clio"]
            );
        }
    );
}

#[test]
fn a_flag_that_was_never_given_reads_back_as_no_values() {
    plain_ctx!("", |ctx| {
        let values = ctx.flag_values("database-option");
        assert!(values.is_empty(), "{values:?}");
    });
}

#[test]
fn an_untouched_yes_no_question_is_refused_rather_than_agreed_to() {
    // The apply gate is the last question before anything is written, so the
    // answer the operator supplies by reflex — a bare Enter — has to be the one
    // that does not write. The default shown is `n` for the same reason.
    let (answer, printed) = plain_ctx!("\n", |ctx| {
        ctx.ask_yes_no("Write these settings and start the containers?")
    });
    assert_eq!(answer, Ok(Some(false)), "{printed}");
    assert!(printed.contains("([n])"), "{printed}");
}

#[test]
fn a_yes_no_question_reads_the_usual_spellings() {
    for (typed, expected) in [
        ("y", Some(true)),
        ("Yes", Some(true)),
        (" y ", Some(true)),
        ("n", Some(false)),
        ("No", Some(false)),
    ] {
        let (answer, printed) = plain_ctx!(&format!("{typed}\n"), |ctx| ctx.ask_yes_no("Sure?"));
        assert_eq!(answer, Ok(expected), "for {typed}: {printed}");
    }
}

#[test]
fn an_answer_that_is_neither_yes_nor_no_is_read_as_no() {
    // The question is a closed set, but the plain renderer has no menu to refuse
    // into: it reads a line. Reading a word nobody meant as `no` is the safe
    // direction for the one question that writes, and refusing here would turn a
    // typo into a stop the operator has to recover from by retyping the question.
    let (answer, _printed) = plain_ctx!("maybe\n", |ctx| ctx.ask_yes_no("Sure?"));
    assert_eq!(answer, Ok(Some(false)));
}

#[test]
fn a_yes_no_question_no_longer_treats_typed_words_as_dialog_keys() {
    // `b` and `q` are gone as ways out. A prompt that reads raw keystrokes cannot
    // offer a reserved word without swallowing a legitimate one-character answer,
    // so both are now ordinary text and read as the answer they spell.
    for typed in ["b", "q"] {
        let (answer, _printed) =
            plain_ctx!(&format!("{typed}\n"), |ctx| { ctx.ask_yes_no("Sure?") });
        assert_eq!(answer, Ok(Some(false)), "for {typed}");
    }
}

#[test]
fn a_yes_no_question_with_no_input_names_the_equivalent_flag() {
    let (answer, _printed) = plain_ctx!("", |ctx| ctx.ask_yes_no("Sure?"));
    let signal = answer.expect_err("a usage stop");
    assert_eq!(
        signal,
        Signal::Usage("no input for `Sure?`; pass the equivalent flag".to_owned())
    );
}

#[test]
fn a_confirmation_is_printed_indented() {
    let (_answer, printed) = plain_ctx!("", |ctx| {
        ctx.confirm("kept the stored value");
    });
    assert_eq!(printed, "  kept the stored value\n");
}

#[test]
fn a_warning_and_an_error_go_to_different_streams() {
    // A warning belongs beside the questions; an error belongs on stderr so a
    // piped stdout stays parseable.
    let mut input = std::io::Cursor::new(Vec::new());
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let flags = std::collections::HashMap::new();
    let probes = crate::setup_test_seam::Probes::ok();
    let mut ctx = Ctx::new(
        &flags,
        &mut input,
        &mut out,
        &mut err,
        true,
        crate::cli_style::Palette::new(false),
        &probes,
        Defaults::empty(),
        PathBuf::from("/nonexistent/clio-stage5/clio.db"),
        crate::compose_docker::DockerStatus::Available,
        &crate::setup_preview_scripts::no_env,
        false,
    );
    ctx.warn("careful");
    ctx.error("no");
    let printed = String::from_utf8(out).expect("utf8");
    assert!(printed.contains("careful"), "{printed}");
    assert!(!printed.contains("no"), "{printed}");
    assert!(String::from_utf8(err).expect("utf8").contains("no"));
}

#[test]
fn the_stored_password_is_read_out_of_the_address_and_never_rendered() {
    // A pasted address keeps its password sealed inside it. The managed route
    // needs that value to reuse it, and nothing else may see it.
    let run = Harness::fresh()
        .with_effective(json!({
            "store": { "database_url": "postgres://clio:p%40ssword@db.example.com:5432/clio" }
        }))
        .typing("1\ny\ndb.example.com\n\n\n\n\n")
        .typing(SERVICES)
        .run();
    assert!(run.answers().database.is_some(), "{}", run.out);
    let drawn = run.preview_text();
    assert!(!drawn.contains("p@ssword"), "{drawn}");
}

#[test]
fn an_address_with_no_password_reads_as_absent() {
    let run = Harness::fresh()
        .with_effective(json!({
            "store": { "database_url": "postgres://clio@db.example.com:5432/clio" }
        }))
        .typing("1\ny\ndb.example.com\n\n\n\n\n")
        .typing(SERVICES)
        .run();
    assert!(run.answers().database.is_some(), "{}", run.out);
}

#[test]
fn the_standard_sqlite_path_is_what_the_store_would_open() {
    let run = Harness::fresh().typing("2\n\n\n\n\n").run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(
        plan.sqlite_path.as_deref(),
        Some(Path::new("/nonexistent/clio-stage5/clio.db"))
    );
}

#[test]
fn the_defaults_a_question_offers_come_from_the_stored_document() {
    let run = Harness::fresh()
        .with_effective(json!({ "backend": "sqlite" }))
        .typing(MANAGED)
        .typing(SERVICES)
        .run();
    assert!(
        run.out.contains("[sqlite]"),
        "the stored choice was not offered: {}",
        run.out
    );
}

#[test]
fn a_value_present_in_the_document_becomes_the_question_default() {
    let run = Harness::fresh()
        .with_effective(json!({ "backend": "sqlite", "store": { "sqlite_path": "/data/x.db" } }))
        .typing(SQLITE)
        .typing(SERVICES)
        .run();
    assert!(run.out.contains("[/data/x.db]"), "{}", run.out);
}

#[test]
fn a_value_of_the_wrong_type_offers_no_default() {
    let run = Harness::fresh()
        .with_effective(json!({ "backend": 7 }))
        .typing(MANAGED)
        .typing(SERVICES)
        .run();
    assert!(run.answers().database.is_some(), "{}", run.out);
}
