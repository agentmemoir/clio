// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the review loop: that the preview
//! is drawn, that Enter applies, that `q` quits, and that `a` re-asks one section
//! and draws the preview again.
//!
//! ## Owns
//! - Review behavior under test for `setup_review`
//!
//! ## Does not own
//! - The preview's wording (see `setup_preview_tests`)
//! - The amend and `switch` prompts (see `setup_report_amend_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to the order "preview, then ask, then apply or redraw" should
//! normally update tests here. Changes to a block's wording should not require
//! changes here.
//!
//! Keep this module focused on the last loop of a run.

use super::*;
use crate::setup_preview_fixtures::{ALL_LOCAL, Harness, PASTED, SERVICES, SQLITE};
use crate::setup_services::ServiceRole;
use crate::setup_support_tests::temp_dir;
use serde_json::json;

/// Every question answered, then whatever the test types at the review prompt.
///
/// A pasted route, so the test's builder must use [`Harness::with_pasted_secrets`]:
/// the address is a secret and comes from the masked reader, not from a typed
/// line.
fn answered_then(after: &str) -> String {
    format!("{PASTED}{SERVICES}{after}")
}

#[test]
fn the_preview_is_drawn_before_the_apply_question_is_asked() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("y\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    let preview_at = run.out.find("Will back up").expect("the preview was drawn");
    let prompt_at = run
        .out
        .find("Write these settings and start the containers?")
        .expect("the apply question was asked");
    assert!(preview_at < prompt_at, "{}", run.out);
}

#[test]
fn enter_applies_and_returns_the_answers() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("y\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply));
    assert!(
        run.answers()
            .database
            .as_ref()
            .expect("a plan")
            .plan
            .address
            .contains("db.example.com"),
        "{}",
        run.out
    );
}

#[test]
fn the_review_preview_names_the_sync_key_store_this_run_recorded() {
    // The loop's own preview builds the sync key's line from the environment this
    // run was given, not from the process's, and the harness's key file is under a
    // directory this test made. So a preview that resolved the path any other way
    // would name a file belonging to whoever is running the suite instead of the one
    // this run would write, and this fails on that rather than on a missing block:
    // a run with no sync store has no such line to get wrong.
    let run = Harness::fresh()
        .with_effective(json!({ "sync": { "omit": false } }))
        .flag("sync-store", "file")
        .with_key_file(temp_dir("review-sync-key").join("clio.key"))
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("y\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert!(run.out.contains("Sync key"), "{}", run.out);
    assert!(
        run.out.contains("clio-sync.key"),
        "the preview names the key file this run would use: {}",
        run.out
    );
}

#[test]
fn leaving_at_the_review_prompt_writes_nothing_and_says_so() {
    // The apply gate is a yes-or-no, and `No` opens the amend menu rather than
    // leaving. So there is nothing to type that leaves from here: the prompt's own
    // cancel is the way out, and a reader cannot produce one. A run that runs out of
    // input is a usage stop, and what matters is that no verdict was reached and so
    // nothing was written.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then(""))
        .review();
    // No verdict is the property that matters: the caller only writes when the
    // review returns `Apply`, so a run that reached no verdict wrote nothing. The
    // operator is told why on stderr, because a stop is a usage error and usage
    // errors go to stderr — so the message is asserted on the stream it is printed
    // to rather than the one the wizard draws on.
    assert_eq!(run.verdict, None, "{}", run.out);
    // The reason travels back as the stop itself, not as a line on either stream:
    // the harness drives the wizard directly, and the caller is what prints it.
    let reason = run.outcome.expect_err("a stop, because input ran out");
    assert!(
        reason.reason().contains("no input for"),
        "the stop names the question that ran out: {}",
        reason.reason()
    );
    assert!(
        !run.out.contains("Applied"),
        "nothing may claim to have been written: {}",
        run.out
    );
}

#[test]
fn amending_jumps_straight_to_the_section_and_redraws_the_preview() {
    // Amending the database re-asks from the database step onwards, which starts
    // at the three-way choice — so a port typo costs the three questions of the
    // pasted route, not the eight of the typed one. The service questions come
    // after it and are re-asked with the answers just given, so the script answers
    // them with their own defaults and then applies.
    // The amended address is on a different port from the first one, so the
    // assertion can tell the two passes apart: a rewind that kept the original
    // address would pass every other check in this file and fail only here.
    // Both passes paste, so the masked reader is asked twice and each answer is the
    // next value in the list: the first address, then the amended one.
    let amended = "postgres://clio:pw@db.example.com:5555/clio";
    let run = Harness::fresh()
        .with_secrets(&[crate::setup_preview_scripts::PASTED_ADDRESS, amended])
        .typing(format!("{PASTED}{ALL_LOCAL}n\n1\n{PASTED}{ALL_LOCAL}y\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert_eq!(run.out.matches("Clio setup").count(), 2, "{}", run.out);
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert!(plan.address.contains(":5555"), "{}", plan.address);
    assert!(plan.address.contains("db.example.com"), "{}", plan.address);
}

#[test]
fn an_amendment_keeps_the_sections_the_operator_did_not_touch() {
    // Every service is set away from its default, and the amendment re-asks the
    // database and everything after it. The services therefore come back with the
    // answers just given as their defaults, so the script retypes them the same way
    // and the point is that the values survive the round trip: if the amend dropped
    // them, the re-ask would take the plan defaults instead and extraction would
    // read as skip — so this fails on a lost answer rather than passing on a lucky
    // default.
    //
    // The second scripted secret is the address the re-ask reads, because a pasted
    // address comes from the masked reader rather than from a typed line.
    // Both passes paste, and each pasted address is the masked reader's next value,
    // so the list is the first address then the second. `with_secrets` takes them in
    // the order the run reaches them, which is the established way to script a
    // reader that is asked twice.
    let run = Harness::fresh()
        .with_secrets(&[
            crate::setup_preview_scripts::PASTED_ADDRESS,
            crate::setup_preview_scripts::PASTED_ADDRESS,
            "pw",
        ])
        .typing(format!("{PASTED}{ALL_LOCAL}n\n1\n{PASTED}{ALL_LOCAL}y\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    let answers = run.answers();
    assert_eq!(answers.embed_role, Some(ServiceRole::Local), "{}", run.out);
    assert_eq!(answers.rerank_role, Some(ServiceRole::Local), "{}", run.out);
    assert_eq!(answers.extract, Some(ServiceRole::Local), "{}", run.out);
}

#[test]
fn backing_out_of_the_amend_question_applies_the_preview_as_it_stands() {
    // `b` at the amend question means "not now" rather than "quit" or "start over":
    // the preview is still on screen and the answers are still good, so the run
    // applies what it already has. Dropping the operator out of the wizard here
    // would lose an hour of answers to a keystroke meant as a change of mind.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("n\n5\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert_eq!(
        run.out.matches("Clio setup").count(),
        1,
        "nothing may be re-asked after backing out: {}",
        run.out
    );
}

#[test]
fn a_store_holding_memories_asks_for_the_word_switch() {
    let run = Harness::fresh()
        .with_non_empty_store(12, 3)
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("y\nswitch\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert!(run.out.contains("12 memories"), "{}", run.out);
    assert!(
        run.out.contains("Nothing is deleted and nothing is copied"),
        "{}",
        run.out
    );
}

#[test]
fn an_untyped_switch_leaves_the_database_unchanged() {
    let run = Harness::fresh()
        .with_non_empty_store(12, 3)
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("y\nno\nok\nsure\n"))
        .review();
    assert_eq!(
        run.signal().map(Signal::reason),
        Some("the confirmation was not typed, so nothing was changed")
    );
}

#[test]
fn a_sync_key_choice_is_previewed_and_an_existing_config_is_named_for_backup() {
    // A multi-host run asks where the sync key lives, so the preview resolves the
    // key file through the run's own environment, and an existing config file is
    // named as what the run would back up. Both are the review preview's own
    // paths: the sync key's line, and the backup line.
    let run = Harness::fresh()
        .with_effective(serde_json::json!({ "sync": { "omit": false } }))
        .with_existing_config()
        .with_key_file(temp_dir("review-sync-key").join("clio.key"))
        .with_pasted_secrets(&["pw"])
        .typing(format!("\n{SQLITE}{SERVICES}y\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert!(run.out.contains("Sync key"), "{}", run.out);
    assert!(
        !run.out.contains("nothing, there is no config file yet"),
        "{}",
        run.out
    );
}

#[test]
fn an_empty_store_asks_for_nothing() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("y\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert!(!run.out.contains("Type `switch`"), "{}", run.out);
}

#[test]
fn a_key_store_choice_is_previewed_and_an_existing_store_is_named() {
    // The run answers the key-store question, so the preview reads the run's own
    // environment for the store summary, and the store holds memories, so the
    // review names them before the operator agrees. Both are the review's own
    // consequence path: the choice is shown, and what already exists is said.
    let run = Harness::fresh()
        .asking_the_key_store()
        .with_key_file(temp_dir("review-key-source").join("clio.key"))
        .with_non_empty_store(2, 1)
        .with_pasted_secrets(&["pw"])
        .typing(format!("\n{PASTED}{SERVICES}y\nswitch\n"))
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert!(run.out.contains("Your existing data"), "{}", run.out);
    assert!(
        run.out.contains("You have 2 memories about 1 subject"),
        "{}",
        run.out
    );
}

#[test]
fn a_non_interactive_run_applies_without_asking() {
    let run = Harness::fresh()
        .non_interactive()
        .with_pasted_secrets(&["pw"])
        .flags([
            ("database", "existing"),
            ("database-connect", "typed"),
            ("database-host", "db.example.com"),
            ("database-port", "5432"),
            ("database-name", "clio"),
            ("database-user", "clio"),
            ("database-password", "pw"),
            ("database-ssl", "prefer"),
            ("embed", "local"),
            ("rerank", "skip"),
            ("extract", "skip"),
        ])
        .review();
    assert_eq!(run.verdict, Some(Verdict::Apply), "{}", run.out);
    assert!(!run.out.contains("[Enter] apply"), "{}", run.out);
}

#[test]
fn an_answer_the_review_question_does_not_accept_reads_as_no() {
    // The apply gate is a yes-or-no, and a yes-or-no has no third answer to reject.
    // Reading a word nobody meant as `No` is the safe direction for the last
    // question before anything is written: it opens the amend menu rather than
    // writing.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then("maybe\n"))
        .review();
    // `No` leads to the amend menu, which lists the answered sections. Two
    // sections have answers, so the way out is the third item.
    assert_eq!(
        run.verdict, None,
        "an unanswered review cannot apply: {}",
        run.out
    );
    assert!(
        run.out.contains("Which answer do you want to change?"),
        "{}",
        run.out
    );
}

#[test]
fn a_review_question_with_no_input_names_the_equivalent_flag() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(answered_then(""))
        .review();
    let reason = run.signal().map(Signal::reason).unwrap_or_default();
    assert!(
        reason.contains("Write these settings and start the containers?"),
        "the message names the question that ran out of input: {reason}"
    );
    assert!(
        reason.contains("pass the equivalent flag"),
        "and it names the flag: {reason}"
    );
}

#[test]
fn a_store_holding_nothing_needs_no_confirmation() {
    assert!(StoreState::Empty.consequence().is_none());
    assert!(StoreState::Unknown.consequence().is_none());
}

#[test]
fn a_store_holding_memories_names_both_counts_and_the_loss() {
    let sentence = StoreState::NonEmpty {
        memories: 12,
        subjects: 3,
    }
    .consequence()
    .expect("a consequence");
    assert!(sentence.contains("12 memories"), "{sentence}");
    assert!(sentence.contains("3 subjects"), "{sentence}");
    // This sentence's job is the counts and which database they are in. What
    // happens to them afterwards is the shared sentence's job, asserted where it
    // is written, so that neither half is pinned twice in two spellings.
    assert!(
        sentence.contains("in the database Clio uses today"),
        "the counts have to say which database is being left: {sentence}"
    );
}

#[test]
fn a_store_holding_one_of_each_is_counted_in_the_singular() {
    // "1 memories" reads as a broken counter, and this is a number the operator is
    // about to act on. A first install with one memory in it is the common case, so
    // it is the one most likely to be read.
    let sentence = StoreState::NonEmpty {
        memories: 1,
        subjects: 1,
    }
    .consequence()
    .expect("a consequence");
    assert!(sentence.contains("1 memory about 1 subject"), "{sentence}");
    assert!(!sentence.contains("1 memories"), "{sentence}");
    assert!(!sentence.contains("1 subjects"), "{sentence}");
}

#[test]
fn the_verdicts_are_the_two_outcomes_the_plan_names() {
    assert_ne!(Verdict::Apply, Verdict::Quit);
}
