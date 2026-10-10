// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of one step's questions: the role
//! choices and the hosted service's three questions.
//!
//! ## Owns
//! - Per-step behavior under test for `setup_step_ask`
//!
//! ## Does not own
//! - Navigation between steps (see `setup_steps_tests`)
//! - The extraction question (see `setup_step_extract_tests`)
//! - The sync key question (see `setup_sync_key_source_tests`)
//! - The preview (see `setup_preview_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to what one step asks should normally update tests here. Changes to
//! the order of the steps should not require changes here.
//!
//! Keep this module focused on one step's questions.

use super::*;
use crate::setup_preview_fixtures::{Harness, PASTED, SERVICES, SQLITE};
use crate::setup_steps::Answers;
use crate::setup_support_tests::temp_dir;
use serde_json::json;

#[test]
fn a_local_role_asks_nothing_more() {
    // A local sidecar's address and model are constants in `compose.yml`, so
    // asking would be asking for something the operator cannot change.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing("1\n1\n1\n")
        .run();
    let answers = run.answers();
    assert_eq!(answers.embed_role, Some(ServiceRole::Local));
    let service = answers.embed.as_ref().expect("a local service");
    assert_eq!(service.url, crate::setup_wizard::LOCAL_EMBED_URL);
    assert_eq!(service.model, crate::setup_wizard::LOCAL_EMBED_MODEL);
    assert!(service.key.is_none(), "a local service needs no key");
    assert!(!run.out.contains("Endpoint URL"), "{}", run.out);
}

#[test]
fn a_hosted_role_asks_for_the_endpoint_model_and_key() {
    // The pasted route asks no password, so the first masked read is the
    // embedding key. The script is one role line plus a URL and a model per
    // hosted service, and the keys come from the masked reader in that order.
    let run = Harness::fresh()
        .with_pasted_secrets(&["sk-embed-value", "sk-rerank-value"])
        .typing(PASTED)
        .typing("2\nhttps://embed.example.test\nbge\n")
        .typing("2\nhttps://rerank.example.test\nbge-rerank\n")
        .typing("1\n")
        .run();
    let answers = run.answers();
    let embed = answers.embed.as_ref().expect("a hosted embedding");
    assert_eq!(embed.role, ServiceRole::Hosted);
    assert_eq!(embed.url, "https://embed.example.test");
    assert_eq!(embed.model, "bge");
    assert_eq!(embed.key.as_deref(), Some("sk-embed-value"));
    let rerank = answers.rerank.as_ref().expect("a hosted rerank");
    assert_eq!(rerank.key.as_deref(), Some("sk-rerank-value"));
}

#[test]
fn a_hosted_key_is_read_through_the_masked_reader_and_never_printed() {
    let secret = "sk-must-not-appear-anywhere-1234";
    let run = Harness::fresh()
        .with_pasted_secrets(&[secret])
        .typing(PASTED)
        .typing("2\nhttps://embed.example.test\nbge\n")
        .typing("1\n1\n")
        .run();
    assert!(!run.out.contains(secret), "the key leaked: {}", run.out);
    assert!(run.out.contains("Embedding API key:"), "{}", run.out);
    assert_eq!(
        run.answers()
            .embed
            .as_ref()
            .expect("a service")
            .key
            .as_deref(),
        Some(secret)
    );
}

#[test]
fn a_remote_endpoint_over_plain_http_is_refused_at_the_question() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw", "k"])
        .typing(PASTED)
        .typing("2\nhttp://embed.example.test\n")
        .drive();
    assert!(
        run.out.contains("must use https for a remote host"),
        "{}",
        run.out
    );
}

#[test]
fn a_failed_endpoint_probe_stops_the_run_and_reports_the_reason() {
    // A probe refusal ends the run rather than re-asking. The blame step-back
    // belongs to the database connection, which names a field; an endpoint
    // refusal names nothing, so there is no field to send the operator back to and
    // re-asking all three would be guessing. What has to survive is the reason,
    // because that is the only thing the operator can act on.
    // The pasted address is a secret now, so the list is the address then the
    // embedding key: without the address the run stops at the address question and
    // the probe is never reached, which is the failure this test exists to catch.
    let run = Harness::fresh()
        .with_pasted_secrets(&["k"])
        .endpoint_fails("the endpoint did not answer")
        .typing(PASTED)
        .typing("2\nhttps://embed.example.test\nbge\n")
        .expect_refusal();
    assert!(
        run.contains("the endpoint did not answer"),
        "the refusal must carry the probe's own reason: {run}"
    );
}

#[test]
fn the_rerank_question_offers_skip_local_and_hosted() {
    // Reranking can be turned off, so its menu has three options and `Skip` is the
    // last of them. Embedding cannot, so its menu has two.
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing("1\n3\n1\n")
        .run();
    assert!(
        run.out.contains("[3] Skip"),
        "the rerank question must offer skip: {}",
        run.out
    );
    assert_eq!(run.answers().rerank_role, Some(ServiceRole::Skip));
}

#[test]
fn a_skipped_service_writes_nothing() {
    let run = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing("1\n3\n1\n")
        .run();
    let entries = crate::setup_preview::planned_entries(run.answers());
    assert!(
        !entries.iter().any(|(path, _)| path.starts_with("rerank")),
        "{entries:?}"
    );
}

#[test]
fn a_role_flag_the_service_cannot_use_is_refused() {
    // A flag is the operator speaking once, so a role the service cannot take has
    // to be refused rather than re-asked: there is no one to ask. The refusal
    // names what is allowed, because the flag's vocabulary is deliberately wider
    // than the question's and a value that reads as reasonable is still refused.
    let reason = Harness::fresh()
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .flag("embed", "sideways")
        .expect_refusal();
    assert!(reason.contains("--embed must be local|online"), "{reason}");
    assert!(
        reason.contains("sideways"),
        "the refusal quotes it: {reason}"
    );
}

#[test]
fn a_hosted_service_with_no_key_and_no_terminal_is_refused() {
    // A hosted service without a key cannot be reached, and a non-interactive run
    // has no question to ask, so the run stops rather than writing an address that
    // will fail on its first request. The key is named because the URL and the
    // model were both answered, and an operator would otherwise expect the service
    // to be fully specified.
    let reason = Harness::fresh()
        .non_interactive()
        .flags([
            ("database", "existing"),
            ("database-connect", "pasted"),
            (
                "database-url",
                "postgres://clio:pw@db.example.com:5432/clio?sslmode=verify-full",
            ),
            ("embed", "online"),
            ("embed-url", "https://embed.example.test"),
            ("embed-model", "text-embedding-3-small"),
        ])
        .expect_refusal();
    assert!(reason.contains("--embed-key is required"), "{reason}");
    assert!(reason.contains("without a terminal"), "{reason}");
}

#[test]
fn a_stored_service_value_becomes_the_question_default() {
    let run = Harness::fresh()
        .with_effective(json!({
            "embed": { "url": "https://old.example.test", "model": "old-model" }
        }))
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing("2\n\n\n")
        .typing("3\n1\n")
        .run();
    assert!(
        run.out.contains("([https://old.example.test])"),
        "the stored endpoint was not offered as the default: {}",
        run.out
    );
    let service = run.answers().embed.as_ref().expect("a service");
    assert_eq!(service.url, "https://old.example.test", "{}", run.out);
    assert_eq!(service.model, "old-model");
}

#[test]
fn the_key_question_is_asked_on_a_first_run_and_reads_the_masked_reader() {
    // The "keep the stored key" branch needs a key this run has already collected,
    // so a first run always asks — and the config's own key does not satisfy it.
    // That is a fact about the code and is pinned here so a change to it is a
    // deliberate one rather than an accident. See the note in the PR body: an
    // operator re-running setup is asked for an API key the config already holds.
    let secret = "sk-typed-this-run";
    let run = Harness::fresh()
        .with_effective(json!({
            "embed": { "url": "https://old.example.test", "model": "old" },
            "credentials": { "embed_api_key": "sk-stored-value" }
        }))
        .with_pasted_secrets(&[secret])
        .typing(PASTED)
        .typing("2\n\n\n")
        .typing("3\n1\n")
        .run();
    let service = run.answers().embed.as_ref().expect("a service");
    assert_eq!(service.key.as_deref(), Some(secret));
    assert!(run.out.contains("Embedding API key:"), "{}", run.out);
    assert!(!run.out.contains(secret), "the key leaked: {}", run.out);
}

#[test]
fn a_role_flag_answers_the_question_without_asking_it() {
    let run = Harness::fresh()
        .flag("embed", "local")
        .with_pasted_secrets(&["pw"])
        .typing(PASTED)
        .typing("1\n1\n")
        .run();
    assert_eq!(run.answers().embed_role, Some(ServiceRole::Local));
    assert!(
        !run.out.contains("Where should embeddings come from?"),
        "{}",
        run.out
    );
}

#[test]
fn each_step_dispatches_to_its_own_group() {
    // A group with no answers in it returns Done, so an empty walk over the plan
    // records exactly the answers each step owns.
    // A fresh install has no key store answer, so the key question is in the plan
    // ahead of the four sections, and neither hosted service is in it because
    // neither role has been answered yet.
    let answers = Answers::default();
    assert_eq!(answers.plan().len(), 5);
    assert!(answers.key_source.is_none());
    assert_eq!(answers.plan()[0], crate::setup_steps::Step::KeySource);
    assert!(answers.embed_role.is_none());
    assert!(answers.rerank_role.is_none());
    assert!(answers.extract.is_none());
}

#[test]
fn the_outcome_of_a_group_distinguishes_a_move_from_a_finish() {
    assert_ne!(Outcome::Done, Outcome::Back);
    assert_ne!(Outcome::Quit, Outcome::Back);
}

#[test]
fn a_single_host_run_is_never_asked_about_the_sync_key() {
    let run = Harness::fresh().typing(SQLITE).typing(SERVICES).run();

    assert!(!run.answers().sync_in_play);
    assert!(run.answers().sync_key_source.is_none());
    assert!(
        !run.out.contains("Where should Clio keep the sync key"),
        "{}",
        run.out
    );
}

#[test]
fn a_multi_host_run_asks_where_the_sync_key_goes() {
    // The sync question is asked before the database, because it shares the
    // once-in-the-life rule with the encryption key, so "1" is its menu answer.
    let run = Harness::fresh()
        .with_effective(json!({ "sync": { "omit": false } }))
        .typing("1\n")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();

    assert!(run.answers().sync_in_play);
    assert_eq!(
        run.answers().sync_key_source,
        Some(clio_config::KeySource::File)
    );
    assert!(
        run.out.contains("Where should Clio keep the sync key"),
        "{}",
        run.out
    );
}

#[test]
fn the_sync_store_flag_answers_the_sync_question_without_a_prompt() {
    let run = Harness::fresh()
        .with_effective(json!({ "sync": { "omit": false } }))
        .flag("sync-store", "keystore")
        .typing(SQLITE)
        .typing(SERVICES)
        .run();

    assert_eq!(
        run.answers().sync_key_source,
        Some(clio_config::KeySource::Keystore)
    );
    assert!(
        !run.out.contains("Where should Clio keep the sync key"),
        "{}",
        run.out
    );
}

#[test]
fn a_recorded_sync_key_store_is_not_asked_again() {
    let dir = temp_dir("sync-key-recorded");
    let key_file = dir.join("clio.key");
    let env = {
        let key_file = key_file.clone();
        move |key: &str| {
            (key == clio_config::KEY_FILE_ENV).then(|| key_file.to_string_lossy().into_owned())
        }
    };
    clio_config::sync_key::record_source(&env, clio_config::KeySource::Keystore)
        .expect("the sync key store is recorded");

    let run = Harness::fresh()
        .with_key_file(&key_file)
        .with_effective(json!({ "sync": { "omit": false } }))
        .typing(SQLITE)
        .typing(SERVICES)
        .run();

    assert_eq!(
        run.answers().sync_key_source,
        Some(clio_config::KeySource::Keystore),
        "the recorded choice is adopted, not re-asked"
    );
    assert!(
        !run.out.contains("Where should Clio keep the sync key"),
        "{}",
        run.out
    );
    let _ = std::fs::remove_dir_all(&dir);
}
