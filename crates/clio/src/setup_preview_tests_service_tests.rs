// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the planned entries themselves:
//! what one service contributes to the config, what a skipped one contributes to
//! nothing, and that a hosted embedding is the only kind that writes a dimension
//! count.
//!
//! ## Owns
//! - The per-service entries and service lists under test for `setup_preview`
//!
//! ## Does not own
//! - The rendered wording an operator reads (see `setup_preview_tests`)
//! - The post-preview questions (see `setup_report_amend_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to what a service contributes to the plan should normally update tests
//! here. Changes to how the plan is drawn should not require changes here.
//!
//! Keep this module focused on the plan's contents rather than its presentation.

use super::*;
use crate::setup_preview_fixtures::{Harness, SERVICES, TYPED};
use crate::setup_services::{Service, ServiceRole};
use crate::setup_steps::Answers;

#[test]
fn the_managed_route_plans_an_address_with_no_password() {
    let answers = Answers {
        key_source: None,
        sync_key_source: None,
        sync_in_play: false,
        database: Some(crate::setup_flows_db::DatabaseAnswers {
            plan: crate::setup_plan_db::managed(
                "127.0.0.1",
                "5432",
                "clio",
                "clio",
                clio_store::SslChoice::Prefer,
                None,
            )
            .expect("a plan"),
            tested: None,
        }),
        embed_role: Some(ServiceRole::Local),
        embed: Some(Service::local("embed")),
        rerank_role: Some(ServiceRole::Skip),
        rerank: Some(Service::skipped()),
        extract: Some(ServiceRole::Skip),
    };
    let entries = planned_entries(&answers);
    let address = entries
        .iter()
        .find(|(path, _)| path == "store.database_url")
        .map(|(_, value)| value.as_str().expect("a string").to_owned())
        .expect("the address is planned");
    assert!(
        address.starts_with("postgres://clio@"),
        "the managed address carries no password: {address}"
    );
    assert!(!address.contains("generated"), "{address}");
    assert_eq!(local_services(&answers), vec!["postgres", "embed"]);
}

#[test]
fn a_skipped_service_writes_nothing_and_owns_nothing() {
    let skipped = Service::skipped();
    let entries = skipped.entries("rerank", "cohere");
    assert!(entries.is_empty(), "{entries:?}");
    assert_eq!(skipped.role, ServiceRole::Skip);
}

#[test]
fn a_hosted_service_writes_its_key_and_owns_it_even_when_it_skips_it() {
    let service = Service {
        role: ServiceRole::Hosted,
        url: "https://embed.example.test".to_owned(),
        model: "m".to_owned(),
        key: None,
        provider: Some("openai".to_owned()),
        dims: None,
        tested: None,
    };
    let entries = service.entries("embed", "openai");
    let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(paths, ["embed.provider", "embed.url", "embed.model"]);
    assert!(Service::watched("embed").contains(&"credentials.embed_api_key"));
}

#[test]
fn the_dimension_count_is_written_only_for_a_hosted_embedding_service() {
    let hosted = Service {
        role: ServiceRole::Hosted,
        url: "https://embed.example.test".to_owned(),
        model: "m".to_owned(),
        key: None,
        provider: None,
        dims: Some(1536),
        tested: None,
    };
    assert!(
        hosted
            .entries("embed", "openai")
            .iter()
            .any(|(path, value)| path == "embed.dims" && *value == json!(1536))
    );
    let mut local = Service::local("embed");
    local.dims = Some(1536);
    assert!(
        // A local sidecar reports its own dimension count, so a stored override
        // would make Clio disagree with it.
        !local
            .entries("embed", "tei")
            .iter()
            .any(|(path, _)| path == "embed.dims")
    );
}

#[test]
fn the_current_document_reaches_the_preview() {
    // The view takes the effective document as a parameter, so a test can prove
    // the renderer is not reading a file of its own and that a config already
    // holding a value turns `new` into `unchanged`.
    let run = Harness::fresh()
        .with_secrets(&["pw"])
        .typing(TYPED)
        .typing(SERVICES)
        .run();
    let fresh = view(run.answers(), &|_| None, None, &|_| None);
    let stored_document = json!({
        "backend": "postgres",
        "store": { "database_url": "postgres://clio:pw@db.example.com:5432/clio?sslmode=prefer" }
    });
    let stored = view(
        run.answers(),
        &|path| crate::setup_report::value_at(&stored_document, path),
        None,
        &|_| None,
    );
    assert_ne!(
        fresh, stored,
        "the current document has to reach the preview"
    );
    assert!(run.preview_text().contains("new"));
}
