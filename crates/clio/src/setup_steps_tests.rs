// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of navigation: the step list, going
//! back, and quitting.
//!
//! ## Owns
//! - Step-list behavior under test for `setup_steps`
//!
//! ## Does not own
//! - The database questions (see `setup_flows_db_tests`)
//! - The preview and the amend loop (see `setup_review_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to the step list or to going back should normally update tests here.
//! Changes to a single question should not.
//!
//! Keep this module focused on navigation.

use super::*;
use crate::setup_preview_fixtures::{Harness, MANAGED, NO_RERANK, SERVICES, SQLITE};
use crate::setup_services::ServiceRole;
use crate::setup_step_ask::Outcome;
use crate::setup_support_tests::temp_dir;
use serde_json::json;

/// SQLite and the three service questions: the shortest complete run.
const ALL_DEFAULTS: &str = "2\n\n\n\n\n";

#[test]
fn the_first_question_is_the_key_store_and_the_database_follows_it() {
    // The order is a decision about what an operator has to think about first. The
    // key store is asked once in the life of an install and cannot be changed
    // afterwards, so it comes before anything that is written to disk.
    assert_eq!(
        Answers::default().plan(),
        vec![
            Step::KeySource,
            Step::Database,
            Step::EmbedRole,
            Step::RerankRole,
            Step::Extract
        ]
    );
}

#[test]
fn an_answered_key_store_is_not_asked_again_by_the_next_run() {
    // The record lives beside the key, and this is the plan reading it: once
    // `key_source` holds a store, the step is not in the list at all, so the
    // question cannot come back on a later run however many times setup is run.
    let answered = Answers {
        key_source: Some(clio_config::KeySource::File),
        ..Answers::default()
    };
    assert_eq!(
        answered.plan(),
        vec![
            Step::Database,
            Step::EmbedRole,
            Step::RerankRole,
            Step::Extract
        ]
    );
}

#[test]
fn the_first_question_defaults_to_managed() {
    let run = Harness::fresh().typing(MANAGED).typing(SERVICES).run();
    assert_eq!(
        run.answers().database.as_ref().expect("a plan").plan.choice,
        crate::setup_plan_db::DatabaseChoice::Managed,
        "{}",
        run.out
    );
}

#[test]
fn the_first_questions_way_out_says_quit_and_leaves_the_run() {
    // The first question is the key store. Its two stores are options 1 and 2, so
    // the way out is 3, and it is an item rather than a key because the prompt reads
    // raw keystrokes: a typed `q` would be swallowed by the prompt library and reach
    // no code at all.
    //
    // A key file is named, so the run has somewhere to record the answer and the
    // marker it would write lands in a temporary directory rather than in the data
    // directory of whoever is running the suite.
    let run = Harness::fresh()
        .asking_the_key_store()
        .with_key_file(temp_dir("steps-quit").join("clio.key"))
        .typing("3\n")
        .drive();
    assert_eq!(run.signal(), Some(&Signal::Quit), "{}", run.out);
    assert!(
        run.out.contains("[3] Quit"),
        "the label says quit: {}",
        run.out
    );
    assert!(run.out.contains("Nothing has been started"), "{}", run.out);
    // Nothing was written: this stage has no write path at all, and the preview
    // is never built on a run that left.
    assert!(run.view.is_none(), "a departure must not build a preview");
}

#[test]
fn a_answered_question_makes_the_next_ones_way_out_say_go_back() {
    // The key store is answered, so the database question has something behind the
    // operator and its way out promises a return rather than a departure. The two
    // labels exist for exactly this: the same option value, two honest descriptions
    // of what it does.
    let run = Harness::fresh()
        .asking_the_key_store()
        .with_key_file(temp_dir("steps-go-back").join("clio.key"))
        .typing("\n")
        .drive();
    let database_at = run
        .out
        .find("Where should your memories live?")
        .expect("the database question was asked");
    let tail = &run.out[database_at..];
    assert!(tail.contains("[4] Go back"), "{}", run.out);
    assert!(!tail.contains("[4] Quit"), "{}", run.out);
}

#[test]
fn the_first_question_is_the_key_store_on_a_machine_that_has_not_chosen() {
    // The two stores are option 1 and 2, and the file store is the recommendation,
    // so an untouched question takes it.
    let run = Harness::fresh()
        .asking_the_key_store()
        .with_key_file(temp_dir("steps-key-asked").join("clio.key"))
        .typing("\n")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();
    assert_eq!(
        run.answers().key_source,
        Some(clio_config::KeySource::File),
        "the file store is the recommendation, so Enter takes it"
    );
    assert!(
        run.out
            .contains("Where should Clio keep its encryption key?"),
        "{}",
        run.out
    );
}

#[test]
fn the_key_store_offers_the_file_first_and_says_why() {
    // The recommendation is a claim about what works everywhere, and the keystore
    // option's own hint says it is the stronger of the two. Recommending the weaker
    // option is only honest if the operator is told that, so both halves are pinned
    // here rather than left to the recommendation implying it.
    let run = Harness::fresh()
        .asking_the_key_store()
        .with_key_file(temp_dir("steps-key-wording").join("clio.key"))
        .typing("\n")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();
    assert!(
        run.out
            .contains("[1] An encrypted key file on this machine"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("works everywhere, including a server or a container"),
        "the file option says why it is the recommendation: {}",
        run.out
    );
    assert!(
        run.out.contains("[2] The operating system's secret store"),
        "{}",
        run.out
    );
    assert!(
        run.out
            .contains("more secure: protected by your login credentials"),
        "the keystore option says it is the stronger one: {}",
        run.out
    );
    // The highlight is on the recommendation, so an untouched question does not
    // silently pick the stronger store on a machine that may not have one.
    assert!(run.out.contains("press Enter for [file]"), "{}", run.out);
}

#[test]
fn the_key_store_is_not_asked_again_once_the_run_has_answered_it() {
    // The same run asked twice would be the bug the question exists to prevent:
    // the flag answers it, so the question is not in the plan at all.
    let run = Harness::fresh().typing(SQLITE).typing(SERVICES).run();
    assert_eq!(
        run.out
            .matches("Where should Clio keep its encryption key?")
            .count(),
        0,
        "{}",
        run.out
    );
}

#[test]
fn going_back_from_a_later_question_re_asks_the_one_before_it() {
    // The database answered, then the way out of the embedding role question, then
    // the database again. `Go back` is a menu item, so it is a number like any
    // other answer rather than a key nothing reads.
    let run = Harness::fresh()
        .typing(format!("{SQLITE}3\n{SQLITE}{SERVICES}"))
        .run();
    assert!(
        run.out
            .contains("Going back. Your answers to Embedding will be discarded."),
        "{}",
        run.out
    );
}

#[test]
fn the_going_back_block_says_going_back_is_free() {
    let run = Harness::fresh()
        .typing(format!("{SQLITE}3\n{SQLITE}{SERVICES}"))
        .run();
    assert!(
        run.out
            .contains("Nothing has been started and nothing has been written yet."),
        "{}",
        run.out
    );
}

#[test]
fn going_back_discards_only_the_answers_above_the_step() {
    // Answer everything, then the way out of the extraction question, then change
    // only the extraction answer. The rerank answer has to survive, because the
    // script has nothing left to retype it with.
    //
    // The extraction menu is skip, local, hosted, then the way out, so the way out
    // is `4`.
    let run = Harness::fresh()
        .typing(format!("{SQLITE}{NO_RERANK}4\n\n\n1\n"))
        .run();
    let answers = run.answers();
    assert_eq!(answers.rerank_role, Some(ServiceRole::Skip), "{}", run.out);
    assert_eq!(answers.extract, Some(ServiceRole::Local), "{}", run.out);
}

#[test]
fn walking_back_three_times_does_not_re_ask_the_questions_above() {
    // Answer everything, go back three times, come forward, and change only the
    // extraction answer. The database questions are asked once: if they were
    // re-asked the script would run out and the run would fail here.
    //
    // Three `Go back` answers in a row: at extraction, then at reranking, then at
    // embedding. The rerank menu is skip, local, hosted, way out, so the way out
    // is `4`; the embedding menu has no skip, so its way out is `3`.
    let run = Harness::fresh()
        .typing(format!("{SQLITE}{NO_RERANK}4\n4\n3\n{SQLITE}1\n"))
        .run();
    assert_eq!(
        run.answers().extract,
        Some(ServiceRole::Local),
        "{}",
        run.out
    );
}

#[test]
fn going_back_inside_one_section_re_asks_only_that_question() {
    // Embedding is a menu followed, when it is hosted, by its detail questions. The
    // way out is taken at a *menu*, because a free-text field has none: a reserved
    // word there would swallow a legitimate one-character answer. Backing out stays
    // inside the section, so nothing is discarded and the section is not named — the
    // operator asked to change a question, not a section.
    //
    // So the script is: SQLite and its file, embedding hosted, its three details,
    // then the rerank menu backed out of. Backing out steps to the question *before*
    // the one that was left, which is the embedding section's last detail field, so
    // the re-ask is offered the answers just given and a blank keeps them. The
    // rerank menu is the fourth option, because reranking also offers `Skip` and the
    // way out always goes last.
    let run = Harness::fresh()
        .with_secrets(&["sk-1", "sk-1"])
        .typing(format!(
            "{SQLITE}2\nhttps://embed.example.test\nm1\n4\n\n\n\n1\n{SERVICES}"
        ))
        .run();
    let answers = run.answers();
    assert_eq!(answers.embed_role, Some(ServiceRole::Hosted), "{}", run.out);
    assert_eq!(answers.rerank_role, Some(ServiceRole::Local), "{}", run.out);
    // A local sidecar still records a service, with its own URL and model, so
    // there is nothing absent to assert on. What the back-out has to show is that
    // the menu was asked again: one question in a section is drawn once, and this
    // one twice.
    assert_eq!(
        run.out.matches("Where should reranking come from?").count(),
        2,
        "the menu was re-asked inside the section: {}",
        run.out
    );
}

#[test]
fn a_stored_database_choice_becomes_the_default() {
    let run = Harness::fresh()
        .with_effective(json!({ "backend": "sqlite", "store": { "sqlite_path": "/data/clio.db" } }))
        .typing(ALL_DEFAULTS)
        .run();
    let plan = &run.answers().database.as_ref().expect("a plan").plan;
    assert_eq!(plan.choice, crate::setup_plan_db::DatabaseChoice::Sqlite);
    assert_eq!(
        plan.sqlite_path.as_deref(),
        Some(std::path::Path::new("/data/clio.db")),
        "{}",
        run.out
    );
}

#[test]
fn the_step_list_skips_the_hosted_step_for_a_local_role() {
    let answers = Answers {
        embed_role: Some(ServiceRole::Local),
        ..Answers::default()
    };
    assert!(!answers.plan().contains(&Step::EmbedService));
}

#[test]
fn the_step_list_includes_the_hosted_step_for_a_hosted_role() {
    let answers = Answers {
        embed_role: Some(ServiceRole::Hosted),
        ..Answers::default()
    };
    assert!(answers.plan().contains(&Step::EmbedService));
}

#[test]
fn the_step_list_includes_the_rerank_detail_step_only_when_hosted() {
    let skipped = Answers {
        rerank_role: Some(ServiceRole::Skip),
        ..Answers::default()
    };
    assert!(!skipped.plan().contains(&Step::RerankService));
    let hosted = Answers {
        rerank_role: Some(ServiceRole::Hosted),
        ..Answers::default()
    };
    assert!(hosted.plan().contains(&Step::RerankService));
}

#[test]
fn extraction_is_the_last_step() {
    let follows = Answers::default().follows(Step::Extract);
    assert!(follows.is_empty(), "{follows:?}");
}

#[test]
fn each_step_belongs_to_the_section_the_plan_names() {
    assert_eq!(Step::Database.section(), "Database");
    assert_eq!(Step::EmbedRole.section(), "Embedding");
    assert_eq!(Step::EmbedService.section(), "Embedding");
    assert_eq!(Step::RerankRole.section(), "Reranking");
    assert_eq!(Step::RerankService.section(), "Reranking");
    assert_eq!(Step::Extract.section(), "Extraction");
}

#[test]
fn each_amend_section_names_the_step_it_re_enters() {
    for section in Amend::ALL {
        let step = Step::first_of(section);
        assert_eq!(step.section(), section.label(), "for {section:?}");
    }
}

#[test]
fn a_contradictory_flag_set_fails_before_any_question() {
    let run = Harness::fresh()
        .flags([("embed", "local"), ("embed-dims", "1536")])
        .preflight_fails("--embed-dims applies only to a hosted embed service")
        .typing("")
        .drive();
    assert_eq!(
        run.signal(),
        Some(&Signal::Usage(
            "--embed-dims applies only to a hosted embed service".to_owned()
        ))
    );
    assert!(!run.out.contains("Where should"), "{}", run.out);
}

#[test]
fn the_two_routes_need_the_number_of_answers_their_scripts_spell_out() {
    // The two constants encode the difference between the routes. A question
    // that moves between them changes a count, and the run then stops with a
    // refusal instead of quietly answering something the test did not script.
    let sqlite = Harness::fresh().typing(ALL_DEFAULTS).run();
    assert_eq!(
        sqlite
            .answers()
            .database
            .as_ref()
            .expect("a plan")
            .plan
            .choice,
        crate::setup_plan_db::DatabaseChoice::Sqlite,
        "{}",
        sqlite.out
    );
    // The counts are the property: one blank line is one answer, so a script that
    // stops matching its route fails here rather than halfway through with a
    // refusal that reads like a bug in the wizard.
    assert_eq!(SQLITE.lines().count(), 2, "the choice and the file");
    // One, and the count is the property: a managed database is a container Clio
    // starts itself, so the host, port, user, name, password and SSL mode are all
    // its to pick. A script with a second line here spends it on the next question.
    assert_eq!(
        MANAGED.lines().count(),
        1,
        "the choice, and nothing else the wizard would override"
    );
}

#[test]
fn an_outcome_is_a_copy_value_the_loop_can_act_on() {
    // `Back` and `Quit` are moves the loop acts on and a finish is neither, so
    // the distinction has to survive being returned from a question.
    assert_ne!(Outcome::Back, Outcome::Quit);
    assert_ne!(Outcome::Done, Outcome::Back);
}
