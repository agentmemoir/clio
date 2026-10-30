// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio ops` text renderings: every field the verify and
//! diagnose JSON lines carry reappears in the human text, and no text rendering
//! leaks JSON syntax.
//!
//! ## Owns
//! - The field-coverage and brace-free assertions for `render_verify`
//! - The field-coverage and brace-free assertions for `render_diagnose`
//!
//! ## Does not own
//! - The renderers under test (`ops_cli_render`)
//! - The report shapes (`clio-ops`)
//! - The end-to-end emission tests (`ops_cli_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a report field is added and not rendered, or when the
//! text rendering starts printing JSON; they never change either.
//!
//! Keep this module focused on the two render functions.

use super::*;
use clio_ops::{Severity, codes};
use clio_store::IntegritySignals;

/// A verify report with one of every shape: a count, a finding, and a note.
fn verify_report() -> VerifyReport {
    VerifyReport {
        ok: false,
        timestamp: "2026-01-02T03:04:05Z".to_owned(),
        scope: "store".to_owned(),
        integrity: IntegritySignals {
            orphan_assoc_edges: 1,
            orphan_triple_items: 2,
            dangling_snapshot_refs: 3,
            dangling_gist_refs: 4,
            orphan_memtree_leaf_items: 5,
        },
        indexable_items: 7,
        dense_rows: 6,
        lexical_rows: 7,
        findings: vec![OpsFinding::new(
            codes::DENSE_COVERAGE_GAP,
            Severity::Error,
            "store",
            "1 dense row(s) behind the indexable items",
        )],
        notes: vec!["other banks carry error findings".to_owned()],
    }
}

/// A diagnose report with none of the optional values set.
fn diagnose_report() -> DiagnoseReport {
    DiagnoseReport {
        ok: true,
        timestamp: "2026-01-02T03:04:05Z".to_owned(),
        backend_reachable: true,
        schema_version: Some("2026-01-01".to_owned()),
        expected_schema_version: "2026-01-01".to_owned(),
        banks: vec!["default".to_owned(), "work".to_owned()],
        vector_index_present: true,
        embedding_model: Some("test-model".to_owned()),
        embedding_dims: Some(8),
        index_mode: Some("ann_vector".to_owned()),
        indexable_items: 7,
        dense_rows: 7,
        lexical_rows: 7,
        pending_jobs: 1,
        retry_jobs: 2,
        embedding_ready: Some(true),
        findings: vec![],
        verify_pointer: "clio ops verify",
    }
}

#[test]
fn render_verify_carries_every_json_field_and_no_json_syntax() {
    let text = render_verify(&verify_report());
    assert!(text.contains("verify: failed"), "{text}");
    assert!(text.contains("scope: store"), "{text}");
    assert!(text.contains("timestamp: 2026-01-02T03:04:05Z"), "{text}");
    for (label, count) in [
        ("orphan_assoc_edges", 1),
        ("orphan_triple_items", 2),
        ("dangling_snapshot_refs", 3),
        ("dangling_gist_refs", 4),
        ("orphan_memtree_leaf_items", 5),
        ("indexable_items", 7),
        ("dense_rows", 6),
        ("lexical_rows", 7),
    ] {
        assert!(text.contains(&format!("{label}: {count}")), "{text}");
    }
    assert!(text.contains("error DENSE_COVERAGE_GAP store:"), "{text}");
    assert!(
        text.contains("note: other banks carry error findings"),
        "{text}"
    );
    assert!(!text.contains('{') && !text.contains('}'), "{text}");
}

#[test]
fn render_verify_empty_findings_and_notes_say_none() {
    let mut report = verify_report();
    report.findings.clear();
    report.notes.clear();
    let text = render_verify(&report);
    assert!(text.contains("findings: none"), "{text}");
    assert!(text.contains("notes: none"), "{text}");
}

#[test]
fn render_verify_ok_reports_the_ok_verdict() {
    let mut report = verify_report();
    report.ok = true;
    assert!(render_verify(&report).contains("verify: ok"));
}

#[test]
fn render_diagnose_carries_every_json_field_and_no_json_syntax() {
    let text = render_diagnose(&diagnose_report());
    assert!(text.contains("diagnose: ok"), "{text}");
    assert!(text.contains("timestamp: 2026-01-02T03:04:05Z"), "{text}");
    assert!(text.contains("backend: reachable"), "{text}");
    assert!(text.contains("schema_version: 2026-01-01"), "{text}");
    assert!(
        text.contains("expected_schema_version: 2026-01-01"),
        "{text}"
    );
    assert!(text.contains("banks: default, work"), "{text}");
    assert!(text.contains("vector_index: present"), "{text}");
    assert!(text.contains("embedding_model: test-model"), "{text}");
    assert!(text.contains("embedding_dims: 8"), "{text}");
    assert!(text.contains("index_mode: ann_vector"), "{text}");
    for (label, count) in [
        ("indexable_items", 7),
        ("dense_rows", 7),
        ("lexical_rows", 7),
        ("pending_jobs", 1),
        ("retry_jobs", 2),
    ] {
        assert!(text.contains(&format!("{label}: {count}")), "{text}");
    }
    assert!(text.contains("embedding_ready: yes"), "{text}");
    assert!(text.contains("deeper checks: clio ops verify"), "{text}");
    assert!(text.contains("findings: none"), "{text}");
    assert!(!text.contains('{') && !text.contains('}'), "{text}");
}

#[test]
fn render_diagnose_absent_values_say_none_or_not_probed() {
    let mut report = diagnose_report();
    report.schema_version = None;
    report.embedding_model = None;
    report.embedding_dims = None;
    report.index_mode = None;
    report.embedding_ready = None;
    report.banks.clear();
    report.backend_reachable = false;
    report.ok = false;
    let text = render_diagnose(&report);
    assert!(text.contains("diagnose: failed"), "{text}");
    assert!(text.contains("backend: unreachable"), "{text}");
    assert!(text.contains("schema_version: none"), "{text}");
    assert!(text.contains("embedding_model: none"), "{text}");
    assert!(text.contains("embedding_dims: none"), "{text}");
    assert!(text.contains("index_mode: none"), "{text}");
    assert!(text.contains("embedding_ready: not probed"), "{text}");
    assert!(text.contains("banks: none"), "{text}");
}

#[test]
fn render_diagnose_lists_each_finding_with_severity_code_and_scope() {
    let mut report = diagnose_report();
    report.findings.push(OpsFinding::new(
        codes::BACKEND_UNREACHABLE,
        Severity::Warning,
        "bank:work",
        "backend ping failed",
    ));
    let text = render_diagnose(&report);
    assert!(text.contains("findings: 1"), "{text}");
    assert!(
        text.contains("warning BACKEND_UNREACHABLE bank:work: backend ping failed"),
        "{text}"
    );
}
