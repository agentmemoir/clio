// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the preview: its block order, its
//! new/unchanged/removed states, its secret masking, and the lines that say what
//! will start, what will be removed, what will be backed up, and what was tested.
//!
//! ## Owns
//! - Preview behavior under test for `setup_preview` and `setup_report`
//!
//! ## Does not own
//! - The per-service entries the plan is built from (see
//!   `setup_preview_service_tests`)
//! - The post-preview questions (see `setup_report_amend_tests`)
//! - The amend loop (see `setup_review_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to what the preview says should normally update tests here. Changes
//! to how it is drawn should not require changes here.
//!
//! Keep this module focused on what an operator reads before agreeing.

use super::*;
use crate::setup_preview_fixtures::{ALL_LOCAL, Harness, MANAGED, PASTED, SERVICES, SQLITE, TYPED};
use crate::setup_services::ServiceRole;
use serde_json::{Value, json};

/// The per-service entries the plan is built from, in their own module.
///
/// Split out rather than declared in `setup_preview`, because that file is at
/// the workspace's line limit and another `#[path]` there would put it over.
///
/// The config path a test's preview names when there is one to back up.
const EXISTING: &str = "/nonexistent/clio-stage5/deployment.json";

/// Draw a view the way the renderer does, so an assertion is about what an
/// operator sees rather than about a struct.
fn render(answers: &Answers, current: &Value) -> String {
    draw(answers, current, None)
}

/// The same, for a run that has a config file to back up.
fn render_with_backup(answers: &Answers, current: &Value) -> String {
    draw(answers, current, Some(EXISTING.to_owned()))
}

fn draw(answers: &Answers, current: &Value, will_back_up: Option<String>) -> String {
    draw_with_env(answers, current, will_back_up, &|_| None)
}

/// The same preview, with the run's environment the key lines resolve against.
///
/// The extract model is checked under the fixture's compose root, which does not
/// exist, so the presence check is about the answers rather than about the machine
/// this suite runs on.
fn draw_with_env(
    answers: &Answers,
    current: &Value,
    will_back_up: Option<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> String {
    let announced = crate::setup_extract_model::announcement(
        answers,
        Some(crate::setup_preview_fixtures::ABSENT_COMPOSE_ROOT),
        env,
    );
    let mut out: Vec<u8> = Vec::new();
    crate::setup_report::preview(
        &mut out,
        crate::cli_style::Palette::new(false),
        &view(
            answers,
            &|path| crate::setup_report::value_at(current, path),
            will_back_up,
            announced.will_fetch,
            announced.will_stop,
            env,
        ),
    )
    .expect("the preview writes to a Vec");
    String::from_utf8(out).expect("utf8")
}

#[test]
fn the_blocks_come_in_the_plans_order() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["sk-1", "sk-2"])
        .typing(PASTED)
        .typing("2\nhttps://embed.example.com\nm\n")
        .typing("2\nhttps://rerank.example.com\nm\n")
        .typing("1\n")
        .run();
    let names: Vec<&str> = run.view().blocks.iter().map(|block| block.name).collect();
    assert_eq!(
        names,
        vec!["Encryption key", "Database", "Embedding", "Reranking"]
    );
}

#[test]
fn a_run_that_skipped_reranking_has_no_reranking_block() {
    // Reranking is declined, and `Skip` is the last of its three options, so the
    // answer is 3. A skipped section has nothing to say.
    let run = Harness::fresh()
        .with_pasted_secrets(&["sk-1"])
        .typing(PASTED)
        .typing("2\nhttps://embed.example.com\nm\n")
        .typing("3\n1\n")
        .run();
    let names: Vec<&str> = run.view().blocks.iter().map(|block| block.name).collect();
    assert_eq!(names, vec!["Encryption key", "Database", "Embedding"]);
    let drawn = run.preview_text();
    assert!(!drawn.contains("Reranking"), "{drawn}");
}

#[test]
fn the_password_row_appears_only_when_the_address_carries_one() {
    // The managed database has no password, so there is no row. Printing one
    // anyway would send an operator looking for a credential that does not exist.
    let managed = Harness::fresh().typing(MANAGED).typing(SERVICES).run();
    let drawn = render(managed.answers(), &json!({}));
    assert!(!drawn.contains("Database password"), "{drawn}");
    assert!(drawn.contains("Database username"), "{drawn}");

    let typed = Harness::fresh()
        .with_secrets(&["a-very-long-password-value"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let drawn = render(typed.answers(), &json!({}));
    assert!(drawn.contains("Database password"), "{drawn}");
    assert!(drawn.contains("(set)"), "{drawn}");
    assert!(!drawn.contains("a-very-long-password-value"), "{drawn}");
}

#[test]
fn an_endpoint_line_says_whether_the_traffic_is_encrypted() {
    // Hosted for both, and hosted reranking is option 2 like hosted embedding.
    let answers = Harness::fresh()
        .with_pasted_secrets(&["sk-1", "sk-2"])
        .typing(PASTED)
        .typing("2\nhttps://embed.example.com\nm\n")
        .typing("2\nhttp://127.0.0.1:8080\nm\n")
        .typing("1\n")
        .run();
    let drawn = render(answers.answers(), &json!({}));
    assert!(
        drawn.contains("https://embed.example.com (encrypted)"),
        "{drawn}"
    );
    assert!(
        drawn.contains("http://127.0.0.1:8080 (this machine only)"),
        "{drawn}"
    );
}

#[test]
fn the_database_line_says_its_encryption_and_where_it_goes() {
    let remote = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let drawn = render(remote.answers(), &json!({}));
    assert!(drawn.contains("encrypted, remote"), "{drawn}");

    let local = Harness::fresh()
        .with_secrets(&["pw"])
        .typing("3\n2\n127.0.0.1\n5432\nclio\n\n\nn\n")
        .typing(SERVICES)
        .run();
    let drawn = render(local.answers(), &json!({}));
    assert!(drawn.contains("prefer, this machine only"), "{drawn}");
}

#[test]
fn the_database_block_shows_the_address_as_the_parts_it_was_built_from() {
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let drawn = render(run.answers(), &json!({}));
    for part in [
        "Database host",
        "Database port",
        "Database name",
        "Database username",
        "Database password",
        "Database SSL mode",
    ] {
        assert!(drawn.contains(part), "the preview omits {part}: {drawn}");
    }
    assert!(drawn.contains("db.example.com"), "{drawn}");
    assert!(drawn.contains("5432"), "{drawn}");
}

#[test]
fn a_value_the_config_already_holds_is_unchanged_and_one_it_does_not_is_new() {
    let current = json!({
        "backend": "postgres",
        "store": { "database_url": "postgres://clio:pw@db.example.com:5432/clio?sslmode=prefer" },
        "embed": { "provider": "tei", "url": "http://127.0.0.1:34311", "model": "BAAI/bge-small-en-v1.5" }
    });
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let drawn = render(run.answers(), &current);
    assert!(drawn.contains("unchanged"), "{drawn}");
    // The rerank endpoint is new, so the run must show a `new` marker too.
    assert!(drawn.contains("new"), "{drawn}");
}

#[test]
fn a_watched_field_the_run_drops_is_shown_as_removed() {
    // A config that holds a hosted key, answered with a local role: the key is
    // going, and the operator has to read that before agreeing.
    let current = json!({
        "credentials": { "rerank_api_key": "sk-old" },
        "rerank": { "url": "https://old.example.com", "model": "old", "provider": "cohere" }
    });
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let current_view = view(
        run.answers(),
        &|path| crate::setup_report::value_at(&current, path),
        None,
        None,
        None,
        &|_| None,
    );
    assert!(
        current_view
            .will_remove
            .iter()
            .any(|name| name.contains("rerank api key")),
        "{:?}",
        current_view.will_remove
    );
    let drawn = render(run.answers(), &current);
    assert!(drawn.contains("removed"), "{drawn}");
    // The rendered heading, not a sentence, so the row and the summary line are
    // both covered by the word the operator actually reads.
    assert!(drawn.contains("Will remove"), "{drawn}");
}

#[test]
fn will_start_names_only_the_services_the_answers_need() {
    let managed = Harness::fresh().typing(MANAGED).typing(ALL_LOCAL).run();
    let answers = managed.answers();
    let choice = answers.database.as_ref().map(|db| db.plan.choice);
    assert_eq!(
        (
            choice,
            answers.embed_role,
            answers.rerank_role,
            answers.extract,
        ),
        (
            Some(crate::setup_plan_db::DatabaseChoice::Managed),
            Some(ServiceRole::Local),
            Some(ServiceRole::Local),
            Some(ServiceRole::Local),
        ),
        "{}",
        managed.out
    );
    assert_eq!(
        managed.view().will_start,
        vec!["postgres", "embed", "rerank", "extract"],
        "{}",
        managed.out
    );

    // A remote database, a hosted embedding service, reranking declined, and a local
    // extractor. The paste is the first scripted secret and the embedding key the
    // second, in the order the run reaches them.
    let hosted = Harness::fresh()
        .with_pasted_secrets(&["sk-1"])
        .typing("3\n1\n")
        .typing("2\nhttps://embed.example.com\nm\n")
        .typing("3\n1\n")
        .run();

    // A remote database starts no database container, which is the whole point of
    // naming the services instead of relying on a Compose profile. Extraction is
    // local here, so it is in the list, and that is a different decision.
    assert!(
        !hosted.view().will_start.contains(&"postgres"),
        "{:?}",
        hosted.view().will_start
    );
    assert_eq!(hosted.view().will_start, vec!["extract"], "{}", hosted.out);
}

#[test]
fn a_fresh_install_says_there_is_no_config_to_back_up() {
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let drawn = render(run.answers(), &json!({}));
    assert!(
        drawn.contains("nothing, there is no config file yet"),
        "{drawn}"
    );
}

#[test]
fn an_existing_install_names_the_backup_path() {
    let run = Harness::fresh()
        .with_existing_config()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    // Both cases, because a first run and an existing install differ only in this
    // line, and getting it wrong in either direction is the whole risk: a run
    // that says it will back up a file that is not there, or one that stays quiet
    // about backing up the file it is about to overwrite.
    assert!(
        run.preview_text().contains("deployment.json"),
        "{}",
        run.preview_text()
    );
    let first = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    assert!(
        first
            .preview_text()
            .contains("nothing, there is no config file yet"),
        "{}",
        first.preview_text()
    );
    // The same answers, drawn with a backup path passed in by the caller rather
    // than probed from the filesystem, so the renderer never decides on its own.
    let drawn = render_with_backup(run.answers(), &json!({}));
    assert!(drawn.contains("deployment.json"), "{drawn}");
}

#[test]
fn an_untested_item_is_visibly_different_from_a_tested_one() {
    // The typed route asks for its password, so the first scripted secret is it
    // and the second is the embedding key.
    let run = Harness::fresh()
        .with_secrets(&["pw", "sk-1"])
        .typing(TYPED)
        .typing("2\nhttps://embed.example.com\nm\n")
        // Reranking local, so its container is not up yet and its line has to say
        // so. Declining reranking instead would leave nothing untested to show.
        .typing("1\n1\n")
        .run();
    let drawn = render(run.answers(), &json!({}));
    assert!(drawn.contains("not tested"), "{drawn}");
    // The hosted endpoint was probed, so its line has a real result.
    assert!(
        drawn.contains("https://embed.example.com answered"),
        "{drawn}"
    );
}

#[test]
fn a_route_with_no_probe_says_so_rather_than_claiming_success() {
    let run = Harness::fresh().typing(SQLITE).typing(SERVICES).run();
    let drawn = render(run.answers(), &json!({}));
    assert!(
        drawn.contains("no connection was tested on this run"),
        "{drawn}"
    );
}

#[test]
fn a_multi_host_run_previews_the_sync_key_under_the_encryption_key() {
    let run = Harness::fresh()
        .with_effective(json!({ "sync": { "omit": false } }))
        .flag("sync-store", "file")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();

    let names: Vec<&str> = run.view().blocks.iter().map(|block| block.name).collect();
    let key = names
        .iter()
        .position(|name| *name == "Encryption key")
        .expect("the encryption key block");
    let sync = names
        .iter()
        .position(|name| *name == "Sync key")
        .expect("the sync key block");
    assert_eq!(sync, key + 1, "the two key stores lead together: {names:?}");

    let drawn = run.preview_text();
    // The block names the recorded store and points at the command that creates
    // the key. It must not read as though the wizard had already written a key
    // file: the apply path records the store choice and nothing else, and the
    // block above it for the data key is different because that key is created.
    assert!(drawn.contains("clio sync keygen"), "{drawn}");
    assert!(drawn.contains("the file store"), "{drawn}");
    assert!(
        !drawn.contains("a sync key file at"),
        "the wizard records the store and does not create the key file: {drawn}"
    );
    // The harness runs with no data directory of its own, so no path resolves and
    // the line says so rather than naming a path from the machine it runs on.
    // The path form is asserted in `setup_sync_key_source_tests`, which injects
    // one.
    assert!(
        drawn.contains("no path resolved"),
        "with no run environment, the line must not invent a path: {drawn}"
    );
}

#[cfg(test)]
#[path = "setup_preview_tests_fetch_tests.rs"]
mod fetch_tests;

#[cfg(test)]
#[path = "setup_preview_tests_service_tests.rs"]
mod service_tests;
