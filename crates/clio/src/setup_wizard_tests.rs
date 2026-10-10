// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the setup apply path: the
//! service selection, the password environment, and the one config write with
//! its summary.
//!
//! ## Owns
//! - Apply-path behavior under test for `setup_wizard`
//!
//! ## Does not own
//! - Production apply logic under test
//! - The scripted end-to-end runs (see `setup_wizard_apply_tests`)
//!
//! ## Boundary
//!
//! Changes to what apply starts, hands over, or writes should normally update
//! tests here. Changes to the CLI surface should not.
//!
//! Keep this module focused on the apply path's values.

use super::*;
use crate::setup_flows_db::DatabaseAnswers;
use crate::setup_plan_db::{DatabasePlan, managed, sqlite};
use crate::setup_preview::local_services;
use crate::setup_preview_scripts::no_env;
use crate::setup_services::{Service, ServiceRole};
use crate::setup_support_tests::{env_at, temp_dir};
use clio_store::SslChoice;
use serde_json::json;
use std::path::Path;

fn answered(
    plan: DatabasePlan,
    embed: Option<Service>,
    rerank: Option<Service>,
    extract: ServiceRole,
) -> Answers {
    Answers {
        // `None` rather than a store: recording one writes a marker file beside
        // the real key on this machine, and these tests are about the values
        // `apply` assembles, not about where the key lives. The question and the
        // record are tested in `setup_key_source_tests` and
        // `clio_config::secret_key_source_tests`.
        key_source: None,
        sync_key_source: None,
        sync_in_play: false,
        database: Some(DatabaseAnswers { plan, tested: None }),
        embed_role: embed.as_ref().map(|service| service.role),
        embed,
        rerank_role: rerank.as_ref().map(|service| service.role),
        rerank,
        extract: Some(extract),
    }
}

fn sqlite_plan(dir: &Path) -> DatabasePlan {
    sqlite(dir.join("memories.db"), None)
}

#[test]
fn the_service_list_is_the_database_choice_plus_the_local_roles() {
    let dir = temp_dir("wizard-services");
    let hosted = answered(
        sqlite_plan(&dir),
        Some(Service::skipped()),
        Some(Service::skipped()),
        ServiceRole::Skip,
    );
    assert!(
        local_services(&hosted).is_empty(),
        "a SQLite install with no local model starts nothing"
    );

    let managed_local = answered(
        managed(
            "127.0.0.1",
            "34310",
            "clio",
            "clio",
            SslChoice::Prefer,
            None,
        )
        .expect("plan"),
        Some(Service::local("embed")),
        Some(Service::local("rerank")),
        ServiceRole::Local,
    );
    assert_eq!(
        local_services(&managed_local),
        vec!["postgres", "embed", "rerank", "extract"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_managed_env_carries_only_the_chosen_port() {
    let dir = temp_dir("wizard-env");
    let managed_plan = managed(
        "127.0.0.1",
        "34310",
        "clio",
        "clio",
        SslChoice::Prefer,
        None,
    )
    .expect("plan");
    let managed_answers = answered(managed_plan, None, None, ServiceRole::Skip);
    // The managed route publishes a port, and it reaches the environment:
    // `compose.yml` reads `${CLIO_POSTGRES_PORT:-34310}` and the `.env` in the
    // compose root is never overwritten, so a chosen port that is not the default
    // has to reach the child process or the container would be published on a
    // port the stored address does not name. The port is read back out of the
    // address, so the value here is the one the store will dial.
    //
    // There is no password in it. The managed database has none, so there is no
    // credential for Compose to carry.
    let env = managed_env(&managed_answers);
    assert_eq!(env, vec![(MANAGED_PORT_VAR.to_owned(), "34310".to_owned())]);
    assert!(
        !env.iter().any(|(key, _)| key == "POSTGRES_PASSWORD"),
        "the managed database has no password to pass: {env:?}"
    );
    assert!(
        managed_answers
            .database
            .as_ref()
            .is_some_and(|database| database.plan.address.contains(":34310")),
        "the port in the environment and the port in the address are one value: {:?}",
        managed_answers.database
    );

    let file_answers = answered(sqlite_plan(&dir), None, None, ServiceRole::Skip);
    let env = managed_env(&file_answers);
    assert!(env.is_empty(), "{env:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_managed_port_variable_is_the_one_compose_substitutes() {
    // The key `managed_env` writes and the key `compose.yml` substitutes have to
    // be one spelling. A difference between them publishes the container on a port
    // the stored address does not name, and the two files disagree silently: the
    // substitution simply falls back to its default.
    assert_eq!(
        MANAGED_PORT_VAR, "CLIO_POSTGRES_PORT",
        "the key is the one compose.yml substitutes and the generated .env writes"
    );
}

#[test]
fn apply_writes_the_paired_keys_and_reports_the_count() {
    let dir = temp_dir("wizard-apply");
    let config = dir.join("deployment.json");
    let plan = sqlite_plan(&dir);
    let db_text = plan.address.clone();
    let answers = answered(plan, Some(Service::local("embed")), None, ServiceRole::Skip);
    let mut rt = clio_config::Runtime::with_env(&env_at(&config));
    let mut out = Vec::new();

    let written = apply(&answers, &mut rt, &config, &mut out, &no_env).expect("the write");
    assert_eq!(written.fields, 6);
    assert_eq!(written.path, config);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&config).expect("read")).expect("json");
    assert_eq!(saved["backend"], json!("sqlite"));
    // `store.database_url` is sealed on disk, so the values are read back
    // through a fresh runtime, the way the next command would.
    let effective = clio_config::Runtime::with_env(&env_at(&config)).effective_raw();
    assert_eq!(effective["backend"], json!("sqlite"));
    assert_eq!(effective["store"]["database_url"], json!(db_text));
    assert_eq!(effective["store"]["sqlite_path"], json!(db_text));
    let text = String::from_utf8(out).expect("utf8");
    assert!(
        text.contains("Applied. 6 settings written to"),
        "the summary counts the entries: {text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
