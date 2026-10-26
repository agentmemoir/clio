// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the read-only `hygiene_audit` scan (`audit.rs`):
//! deterministic ranking, read-only behavior, pagination, secret
//! masking, and the scan-budget `incomplete` signal. SQLite backend;
//! the Postgres parity probe lives in `clio-store`'s `pg_hygiene_tests`.
//!
//! ## Owns
//! - Test coverage for ranking determinism, read-only behavior, pagination,
//!   secret masking, and the scan-budget incomplete signal
//!
//! ## Does not own
//! - Edge-case detector and staleness math (`audit_edge_tests.rs`)
//! - The audit behavior itself (`audit.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the audit scan changes; they never change it.
//!
//! Keep this module focused on `hygiene_audit` scan tests.

use crate::audit::{AuditArgs, DEFAULT_MAX_SCAN, DEFAULT_MIN_SCORE, hygiene_audit};
use crate::clean::{CleanAction, CleanArgs, clean, targets_from_candidates};
use clio_config::ranking::HygieneWeights;
use clio_store::{HygieneStore, LocalDevKms, SqliteStore, Store};
use clio_types::{Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory};
use std::sync::{Arc, LazyLock};

fn store() -> Arc<SqliteStore> {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()))
        .map(Arc::new)
        .expect("open sqlite")
}

fn item(id: &str, bank: &str, gist: &str, admission: f64, created_at: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(admission),
        source_ref: Some("turn_9".into()),
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: created_at.into(),
        valid_until: None,
        tx_from: created_at.into(),
        tx_until: None,
        created_at: created_at.into(),
        updated_at: created_at.into(),
        context: None,
    }
}

static WEIGHTS: LazyLock<HygieneWeights> = LazyLock::new(HygieneWeights::default);

fn audit_args<'a>(
    store: &'a dyn Store,
    bank: &'a str,
    now: &'a str,
    min_score: f64,
    limit: usize,
    offset: usize,
) -> AuditArgs<'a> {
    AuditArgs {
        store,
        bank,
        now,
        min_score,
        limit,
        offset,
        weights: &WEIGHTS,
        stale_half_life_days: 90.0,
        max_scan: DEFAULT_MAX_SCAN,
    }
}

fn seed(store: &SqliteStore, bank: &str) {
    let s = store as &dyn Store;
    // fresh, high-utility, clean
    s.create_memory_item(
        &item(
            "itm-a",
            bank,
            "service listens on 8080",
            0.9,
            "2026-09-20T00:00:00Z",
        ),
        "actor",
    )
    .unwrap();
    // stale + never-retrieved noise
    s.create_memory_item(
        &item(
            "itm-b",
            bank,
            "random scratch note number two",
            0.0,
            "2026-01-01T00:00:00Z",
        ),
        "actor",
    )
    .unwrap();
    // secret leak
    s.create_memory_item(
        &item(
            "itm-c",
            bank,
            "api_key: sk-live-abcdefgh1234",
            0.5,
            "2026-08-01T00:00:00Z",
        ),
        "actor",
    )
    .unwrap();
    // terminal spam
    s.create_memory_item(
        &item(
            "itm-d",
            bank,
            "$ ls\n$ cd /tmp\n$ rm -rf build",
            0.5,
            "2026-07-01T00:00:00Z",
        ),
        "actor",
    )
    .unwrap();
}

#[test]
fn t21_01_ranking_is_deterministic_and_descending() {
    let s = store();
    let bank = "bank-audit";
    seed(&s, bank);
    let first = hygiene_audit(&audit_args(&*s, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    let second = hygiene_audit(&audit_args(&*s, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    assert_eq!(
        first.candidates, second.candidates,
        "identical fixtures + config => identical rank"
    );
    assert_eq!(first.scanned, 4);
    let ids: Vec<&str> = first
        .candidates
        .iter()
        .map(|c| c.item_id.as_str())
        .collect();
    assert!(
        ids.contains(&"itm-a")
            && ids.contains(&"itm-b")
            && ids.contains(&"itm-c")
            && ids.contains(&"itm-d")
    );
    let scores: Vec<f64> = first.candidates.iter().map(|c| c.noise_score).collect();
    let mut sorted = scores.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap().then(std::cmp::Ordering::Greater));
    assert!(scores == sorted, "candidates descend by noise score");
    // tie-break determinism: equal scores order by item_id ascending
    for w in scores.windows(2) {
        if (w[0] - w[1]).abs() < 1e-9 {
            let i = scores.iter().position(|x| (x - w[0]).abs() < 1e-9).unwrap();
            assert!(first.candidates[i].item_id <= first.candidates[i + 1].item_id);
        }
    }
}

#[test]
fn t21_01_secret_candidate_ranks_top_with_redacted_preview() {
    let s = store();
    let bank = "bank-secret";
    seed(&s, bank);
    let report = hygiene_audit(&audit_args(
        &*s,
        bank,
        "2026-09-20T00:00:00Z",
        DEFAULT_MIN_SCORE,
        50,
        0,
    ))
    .unwrap();
    let secret = report
        .candidates
        .iter()
        .find(|c| c.item_id == "itm-c")
        .expect("secret candidate present");
    assert!(secret.noise_score >= 0.90);
    assert!(
        secret
            .noise_reasons
            .iter()
            .any(|r| r == "secret_credential")
    );
    assert_eq!(secret.suggested_action.as_deref(), Some("discard"));
    assert!(
        secret.preview.as_deref() == Some("[REDACTED]"),
        "secret preview redacted"
    );
    assert_eq!(
        report
            .candidates
            .first()
            .expect("secret candidate ranked")
            .item_id,
        "itm-c",
        "secret floor puts the credential leak first"
    );
}

#[test]
fn audit_is_read_only_and_min_score_filters() {
    let s = store();
    let bank = "bank-ro";
    seed(&s, bank);
    let before = s
        .inspect_items(&{
            let mut f = clio_store::ItemInspectFilter::new(bank);
            f.limit = 10;
            f
        })
        .unwrap();
    let report = hygiene_audit(&audit_args(
        &*s,
        bank,
        "2026-09-20T00:00:00Z",
        DEFAULT_MIN_SCORE,
        50,
        0,
    ))
    .unwrap();
    assert!(
        !report.candidates.is_empty(),
        "a seeded bank yields audit candidates"
    );
    // Read-only: no flag/archive/discard writes, no hygiene log rows.
    let after = s
        .inspect_items(&{
            let mut f = clio_store::ItemInspectFilter::new(bank);
            f.limit = 10;
            f
        })
        .unwrap();
    assert_eq!(before, after);
    let logs = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert!(logs.is_empty(), "{logs:?}");
    // min_score floor: a high floor drops everything but the secret hit.
    let strict =
        hygiene_audit(&audit_args(&*s, bank, "2026-09-20T00:00:00Z", 0.90, 50, 0)).unwrap();
    assert_eq!(strict.candidates.len(), 1);
    assert_eq!(strict.candidates[0].item_id, "itm-c");
}

#[test]
fn pagination_is_stable_and_bank_scoped() {
    let s = store();
    seed(&s, "bank-page");
    s.create_memory_item(
        &item(
            "itm-x",
            "bank-other",
            "secret password: hunter2abcdefgh",
            0.0,
            "2026-09-01T00:00:00Z",
        ),
        "actor",
    )
    .unwrap();
    let full = hygiene_audit(&audit_args(
        &*s,
        "bank-page",
        "2026-09-20T00:00:00Z",
        0.0,
        50,
        0,
    ))
    .unwrap();
    let page1 = hygiene_audit(&audit_args(
        &*s,
        "bank-page",
        "2026-09-20T00:00:00Z",
        0.0,
        2,
        0,
    ))
    .unwrap();
    let page2 = hygiene_audit(&audit_args(
        &*s,
        "bank-page",
        "2026-09-20T00:00:00Z",
        0.0,
        2,
        2,
    ))
    .unwrap();
    assert_eq!(page1.candidates.len(), 2);
    assert_eq!(page2.candidates.len(), 2);
    assert_eq!(full.total_candidates, 4);
    let joined: Vec<String> = page1
        .candidates
        .iter()
        .chain(page2.candidates.iter())
        .map(|c| c.item_id.clone())
        .collect();
    let expected: Vec<String> = full.candidates.iter().map(|c| c.item_id.clone()).collect();
    assert_eq!(joined, expected, "offset/limit pages the ranked list");
    // Bank isolation: only the scoped bank is scanned.
    let empty = hygiene_audit(&audit_args(
        &*s,
        "bank-empty",
        "2026-09-20T00:00:00Z",
        0.0,
        50,
        0,
    ))
    .unwrap();
    assert_eq!(empty.scanned, 0);
}

#[test]
fn ordinary_long_id_record_is_not_a_secret_credential() {
    let s = store();
    let bank = "bank-long-id";
    // The failure record whose hyphenated identifiers are long enough to look
    // token-shaped: it rides along as the snapshot half of the scanned text.
    let record = r#"{"id":"fr-agents-clb016-long-id-probe-task-with-long-identifier-1792665330634632000-1792665446249689000-1","item_id":"failure-item-agents-clb016-long-id-probe-task-with-long-identifier-1792665330634632000-1792665446249679000-0","task_id":"clb016-long-id-probe-task-with-long-identifier-1792665330634632000","what_failed":"clb016 ordinary failure text","lesson":"clb016 ordinary lesson text"}"#;
    let gist = "clb016 ordinary failure text";
    let mut itm = item("itm-long", bank, gist, 0.5, "2026-09-01T00:00:00Z");
    itm.snapshot = Some(serde_json::from_str(record).expect("failure record json"));
    s.create_memory_item(&itm, "actor").unwrap();
    let report = hygiene_audit(&audit_args(&*s, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0)).unwrap();
    let c = report
        .candidates
        .iter()
        .find(|c| c.item_id == "itm-long")
        .expect("candidate");
    assert!(
        !c.noise_reasons.contains(&"secret_credential".to_owned()),
        "an ordinary long id is not classified as a credential"
    );
    assert_ne!(c.suggested_action.as_deref(), Some("discard"));
    assert!(c.noise_score < 0.90, "score stays below the discard floor");
    assert!(
        (0.40..0.70).contains(&c.noise_score),
        "proxy score sits in the flag band"
    );
    let preview = c.preview.as_deref().expect("preview");
    assert!(
        preview != "[REDACTED]",
        "a proxy preview is not fully redacted"
    );
    assert!(
        preview.contains("clb016 ordinary"),
        "the proxy preview keeps the gist text"
    );
}

#[test]
fn invalid_args_fail_closed() {
    let s = store();
    for args in [
        audit_args(&*s, "", "2026-09-20T00:00:00Z", 0.4, 50, 0),
        audit_args(&*s, "b", "2026-09-20T00:00:00Z", 1.5, 50, 0),
        audit_args(&*s, "b", "2026-09-20T00:00:00Z", 0.4, 0, 0),
        audit_args(&*s, "b", "2026-09-20T00:00:00Z", 0.4, 201, 0),
    ] {
        assert!(hygiene_audit(&args).is_err(), "expected rejection");
    }
}

#[test]
fn audit_feeds_clean_targets_end_to_end() {
    let s = store();
    let bank = "bank-e2e";
    seed(&s, bank);
    let report = hygiene_audit(&audit_args(
        &*s,
        bank,
        "2026-09-20T00:00:00Z",
        DEFAULT_MIN_SCORE,
        50,
        0,
    ))
    .unwrap();
    let targets = targets_from_candidates(&report.candidates);
    let ctx = OpContext::new(Bank::new(bank), clio_types::Actor::System);
    let outcome = clean(&CleanArgs {
        store: &*s,
        hygiene: &*s,
        ctx: &ctx,
        targets: &targets,
        action: CleanAction::Keep,
        confirm: false,
        reason: "",
        timestamp: "2026-09-20T00:00:00Z",
        batch_id: "e2e-1",
        note: None,
    })
    .unwrap();
    assert!(outcome.dry_run);
    assert!(
        outcome.would_discard >= 1,
        "secret candidate suggests discard"
    );
}

#[test]
fn scan_budget_sets_incomplete_and_stays_read_only() {
    let s = store();
    let bank = "bank-budget";
    seed(&s, bank);
    // Budget of 1 over a 5-item bank: one item scored, `incomplete` set.
    let mut a = audit_args(&*s, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0);
    a.max_scan = 1;
    let partial = hygiene_audit(&a).unwrap();
    assert!(partial.incomplete, "budget smaller than the bank");
    assert_eq!(partial.scanned, 1);
    assert!(partial.total_candidates <= 1);
    assert!(
        partial
            .candidates
            .iter()
            .all(|c| c.preview.as_deref().is_none_or(|p| !p.contains("sk-live"))),
        "masked previews on partial reports too"
    );
    // Budget at/above the bank size: full scan, no incomplete signal.
    let mut ample = audit_args(&*s, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0);
    ample.max_scan = 1000;
    let full = hygiene_audit(&ample).unwrap();
    assert!(!full.incomplete);
    assert_eq!(full.scanned, 4, "seed() creates four items");
    // Exactly-at-budget banks complete: a budget equal to the bank size does
    // not invent a truncation signal.
    let mut exact = audit_args(&*s, bank, "2026-09-20T00:00:00Z", 0.0, 50, 0);
    exact.max_scan = full.scanned;
    let exact_report = hygiene_audit(&exact).unwrap();
    assert!(!exact_report.incomplete);
    assert_eq!(exact_report.scanned, full.scanned);
    // Read-only either way (partial signal never mutates).
    let logs = s.list_hygiene_logs(Some(bank), 50, 0).unwrap();
    assert!(logs.is_empty(), "audit writes nothing, complete or partial");
}
