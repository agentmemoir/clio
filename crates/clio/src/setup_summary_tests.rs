// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the applied summary's wording: the
//! count line, the database line's host, port, and locality, the service lines
//! and their parenthetical, and the omission of a skipped service.
//!
//! ## Owns
//! - Summary-rendering behavior under test for `setup_summary`
//!
//! ## Does not own
//! - The write that precedes the summary (see `setup_config_tests`)
//! - The end-to-end run that prints it (see `setup_wizard_apply_flow_tests`)
//!
//! ## Boundary
//!
//! Changes to the applied summary's wording should normally update tests here.
//! Changes to what is written should not.
//!
//! Keep this module focused on the summary's text.

use super::*;
use crate::setup_flows_db::DatabaseAnswers;
use crate::setup_plan_db::{DatabasePlan, existing, managed, sqlite};
use crate::setup_services::Service;
use crate::setup_steps::Answers;
use clio_store::SslChoice;
use std::path::PathBuf;

fn answered(plan: DatabasePlan, embed: Option<Service>, rerank: Option<Service>) -> Answers {
    Answers {
        key_source: None,
        sync_key_source: None,
        sync_in_play: false,
        database: Some(DatabaseAnswers { plan, tested: None }),
        embed_role: embed.as_ref().map(|service| service.role),
        embed,
        rerank_role: rerank.as_ref().map(|service| service.role),
        rerank,
        extract: Some(ServiceRole::Skip),
    }
}

fn written(fields: usize) -> Written {
    Written {
        path: PathBuf::from("/tmp/clio-test/deployment.json"),
        backup: None,
        fields,
    }
}

fn render(answers: &Answers, fields: usize) -> String {
    let mut out = Vec::new();
    applied(answers, &written(fields), &mut out).expect("a Vec writes");
    String::from_utf8(out).expect("utf8")
}

#[test]
fn a_managed_loopback_run_names_the_parts_and_the_locality() {
    let plan = managed(
        "127.0.0.1",
        "34310",
        "clio",
        "clio",
        SslChoice::Prefer,
        None,
    )
    .expect("a valid managed plan");
    let answers = answered(plan, Some(Service::local("embed")), None);
    let text = render(&answers, 5);

    assert!(
        text.contains("Applied. 5 settings written to /tmp/clio-test/deployment.json"),
        "{text}"
    );
    assert!(
        text.contains("Database: Postgres at 127.0.0.1:34310, encrypted: no, this machine only"),
        "{text}"
    );
    assert!(
        text.contains("Embedding: http://127.0.0.1:34311 (this machine only)"),
        "{text}"
    );
    assert!(
        !text.contains("Reranking:"),
        "a skip prints no line: {text}"
    );
    assert!(text.contains("Encryption key: "), "{text}");
    assert!(text.contains("clio status"), "{text}");
    assert!(
        !text.contains("generated-password"),
        "the summary never prints a secret: {text}"
    );
}

#[test]
fn a_remote_encrypted_run_names_remote_and_encrypted() {
    let plan = existing(
        "postgres://clio:pw@db.example.com:5432/clio?sslmode=require".to_owned(),
        SslChoice::Require,
        Vec::new(),
        None,
    )
    .expect("a valid existing plan");
    let answers = answered(
        plan,
        Some(Service::local("embed")),
        Some(Service::skipped()),
    );
    let text = render(&answers, 6);

    assert!(
        text.contains("Database: Postgres at db.example.com:5432, encrypted, remote"),
        "{text}"
    );
    assert!(
        text.contains("Embedding: http://127.0.0.1:34311 (this machine only)"),
        "{text}"
    );
    assert!(
        !text.contains("Reranking:"),
        "a skip prints no line: {text}"
    );
    assert!(
        !text.contains(":pw@"),
        "the password is never shown: {text}"
    );
}

#[test]
fn a_sqlite_run_prints_the_file_and_local_encryption() {
    let path = PathBuf::from("/tmp/clio-test/memories.db");
    let answers = answered(sqlite(path.clone(), None), None, None);
    let text = render(&answers, 3);

    assert!(
        text.contains(&format!(
            "Database: SQLite file at {}, not encrypted, this machine only",
            path.display()
        )),
        "{text}"
    );
    assert!(!text.contains("Embedding:"), "{text}");
}
