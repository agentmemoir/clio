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
use crate::setup_plan_db::{DatabaseChoice, DatabasePlan, existing, managed, sqlite};
use crate::setup_services::Service;
use crate::setup_steps::Answers;
use clio_store::SslChoice;
use std::fs;
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
        text.contains(
            "Database: Postgres at 127.0.0.1:34310, encrypted and verified when offered, this \
             machine only"
        ),
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
        "postgres://clio:pw@db.example.com:5432/clio?sslmode=verify-full".to_owned(),
        SslChoice::VerifyFull,
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
        text.contains("Database: Postgres at db.example.com:5432, encrypted and verified, remote"),
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
fn the_applied_service_line_never_shows_url_userinfo_credentials() {
    let plan = managed(
        "127.0.0.1",
        "34310",
        "clio",
        "clio",
        SslChoice::Prefer,
        None,
    )
    .expect("a valid managed plan");
    let mut service = Service::local("embed");
    service.url = "http://user:pw@127.0.0.1:18080".to_owned();
    let text = render(&answered(plan, Some(service), None), 5);
    assert!(
        text.contains("Embedding: http://***@127.0.0.1:18080 (this machine only)"),
        "{text}"
    );
    assert!(!text.contains("pw"), "the password is shown: {text}");

    let plan = managed(
        "127.0.0.1",
        "34310",
        "clio",
        "clio",
        SslChoice::Prefer,
        None,
    )
    .expect("a valid managed plan");
    let unchanged = render(&answered(plan, Some(Service::local("embed")), None), 5);
    assert!(
        unchanged.contains("Embedding: http://127.0.0.1:34311 (this machine only)"),
        "{unchanged}"
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

#[test]
fn a_hosted_rerank_row_is_labelled_encrypted() {
    let plan = managed(
        "127.0.0.1",
        "34310",
        "clio",
        "clio",
        SslChoice::Prefer,
        None,
    )
    .expect("a valid managed plan");
    let mut rerank = Service::local("rerank");
    rerank.role = ServiceRole::Hosted;
    rerank.url = "https://rerank.example.com/v1".to_owned();
    let text = render(
        &answered(plan, Some(Service::local("embed")), Some(rerank)),
        6,
    );

    assert!(
        text.contains("Reranking: https://rerank.example.com/v1 (encrypted)"),
        "{text}"
    );
    assert!(
        text.contains("Embedding: http://127.0.0.1:34311 (this machine only)"),
        "{text}"
    );
}

#[test]
fn a_postgres_address_the_parser_refuses_still_hides_the_password() {
    let plan = DatabasePlan {
        choice: DatabaseChoice::Existing,
        address: "postgres://clio:pw@db.example.com:5432/clio?sslmode=bogus".to_owned(),
        sqlite_path: None,
        ssl: SslChoice::VerifyFull,
        extra: Vec::new(),
        previous_store: None,
    };
    let text = render(&answered(plan, None, None), 5);

    assert!(
        text.contains(
            "Database: Postgres at postgres://***@db.example.com:5432/clio?sslmode=bogus, \
             encryption could not be read"
        ),
        "{text}"
    );
    assert!(!text.contains(":pw@"), "the password is shown: {text}");
}

/// One test's own directory, removed when the guard drops.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("clio-setup-summary-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("scratch directory can be created");
        Self { root }
    }

    fn env_path(&self) -> PathBuf {
        self.root.join(".env")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stale_note(dir: &Scratch) -> String {
    let root = dir.root.to_string_lossy().into_owned();
    let mut out = Vec::new();
    stale_env_note(Some(&root), &mut out).expect("a Vec writes");
    String::from_utf8(out).expect("utf8")
}

#[test]
fn a_legacy_compose_password_line_is_reported_without_rewriting_the_file() {
    let dir = Scratch::new("legacy");
    let original = "POSTGRES_PASSWORD=old-secret\nCOMPOSE_PROJECT_NAME=clio\n";
    fs::write(dir.env_path(), original).expect("a fixture can be written");
    let text = stale_note(&dir);

    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.contains("still sets POSTGRES_PASSWORD"), "{text}");
    assert!(
        text.contains(&dir.env_path().display().to_string()),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(dir.env_path()).expect("the fixture can be read"),
        original,
        "the note never rewrites the file"
    );
}

#[test]
fn a_compose_env_without_the_password_is_not_mentioned() {
    let dir = Scratch::new("clean");
    fs::write(dir.env_path(), "COMPOSE_PROJECT_NAME=clio\n").expect("a fixture can be written");

    assert_eq!(stale_note(&dir), "");
}

#[test]
fn an_absent_compose_env_is_not_mentioned() {
    let dir = Scratch::new("absent");

    assert_eq!(stale_note(&dir), "");
}
