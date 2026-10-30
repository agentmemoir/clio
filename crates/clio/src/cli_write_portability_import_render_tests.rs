// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `import` human text view: the summary line for each
//! run shape, the skipped-as-invalid naming tail, and field-by-field parity
//! with the JSON payload (`written`, `created_ids`, `rejection_samples`,
//! `notes`).
//!
//! ## Owns
//! - The live zero-create, zero-overwrite render line of the `import` view
//! - The skipped-as-invalid tail of the live render lines
//! - The detail lines and their parity with the JSON report
//!
//! ## Does not own
//! - The command under test (`cli_write_portability`)
//! - Import behaviour and report shape (see `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_write_group::WriteGroup;
use serde_json::json;

fn render(payload: &serde_json::Value) -> String {
    crate::cli_write_portability::PortabilityWriteGroup.render(
        "import",
        payload,
        crate::cli_style::Palette::new(false),
    )
}

/// A live run that created and overwrote nothing says nothing was restored.
#[test]
fn render_says_a_live_zero_write_import_restored_nothing() {
    let line = render(
        &json!({"ok": true, "report": {"dry_run": false, "applied": true, "would_create": 0,
                 "would_skip": 4840, "would_reject": 0, "would_overwrite": 0}}),
    );
    assert_eq!(
        line,
        "import: nothing restored: created 0, overwrote 0, skipped 4840, rejected 0\n\
         dry_run: false\n\
         applied: true"
    );
}

/// A live run that skipped one entity as invalid names it, with code and reason.
#[test]
fn render_names_the_row_a_live_import_skipped_as_invalid() {
    let line = render(
        &json!({"ok": true, "report": {"dry_run": false, "applied": true, "would_create": 1,
                 "would_skip": 0, "would_reject": 0, "would_overwrite": 0,
                 "would_skip_invalid": 1,
                 "skipped_invalid": [{"id": "itm-legacy#bank-a", "code": "out_of_range",
                                      "reason": "gist exceeds 250 characters (1079)"}]}}),
    );
    assert_eq!(
        line,
        "import: created 1, skipped 0, rejected 0, overwrote 0, skipped invalid 1: \
         itm-legacy#bank-a (out_of_range: gist exceeds 250 characters (1079))\n\
         dry_run: false\n\
         applied: true"
    );
}

/// An all-invalid run names the count, never "skipped 0", and caps the list.
#[test]
fn render_of_an_all_invalid_import_counts_every_skipped_row() {
    let line = render(
        &json!({"ok": true, "report": {"dry_run": false, "applied": true, "would_create": 0,
                 "would_skip": 0, "would_reject": 0, "would_overwrite": 0,
                 "would_skip_invalid": 5,
                 "skipped_invalid": [
                     {"id": "itm-1#bank-a", "code": "out_of_range", "reason": "too long"},
                     {"id": "itm-2#bank-a", "code": "out_of_range", "reason": "too long"},
                     {"id": "itm-3#bank-a", "code": "out_of_range", "reason": "too long"}]}}),
    );
    assert_eq!(
        line,
        "import: nothing restored: created 0, overwrote 0, skipped 0, rejected 0, \
         skipped invalid 5: itm-1#bank-a (out_of_range: too long); \
         itm-2#bank-a (out_of_range: too long); itm-3#bank-a (out_of_range: too long) \
         and 2 more\n\
         dry_run: false\n\
         applied: true"
    );
}

/// The representative payload: the `written` list and a report carrying every
/// `ImportReport` field, so parity is asserted over each field name.
fn payload() -> serde_json::Value {
    json!({
        "ok": true,
        "written": [{"bank": "bank-a", "id": "itm-1"}, {"bank": "bank-a", "id": "itm-2"}],
        "report": {
            "would_create": 2,
            "would_skip": 1,
            "would_reject": 1,
            "would_overwrite": 1,
            "would_skip_invalid": 1,
            "skipped_invalid": [{"id": "itm-x#bank-a", "code": "out_of_range", "reason": "too long"}],
            "created_ids": ["itm-1", "itm-2"],
            "rejection_samples": ["rejected: category not accepted (profile)"],
            "dry_run": false,
            "applied": true,
            "notes": ["restored 2 items into bank-a"]
        }
    })
}

/// Every JSON field of the payload is present in the text view. The summary
/// line renders the four would-counts in its own words (created/skipped/
/// rejected/overwrote) and the skipped-invalid tail renders the two
/// skipped-invalid fields; the detail lines name the rest. `ok` is the
/// success envelope the exit code already carries, so it gets no text line.
#[test]
fn text_carries_every_field_the_json_payload_carries() {
    let payload = payload();
    let text = render(&payload);
    for (key, shown) in [
        ("written", "written: bank-a/itm-1, bank-a/itm-2"),
        ("would_create", "created 2"),
        ("would_skip", "skipped 1"),
        ("would_reject", "rejected 1"),
        ("would_overwrite", "overwrote 1"),
        ("would_skip_invalid", "skipped invalid 1"),
        ("skipped_invalid", "itm-x#bank-a (out_of_range: too long)"),
        ("dry_run", "dry_run: false"),
        ("applied", "applied: true"),
        ("created_ids", "created_ids: itm-1, itm-2"),
        (
            "rejection_samples",
            "rejection_samples: rejected: category not accepted (profile)",
        ),
        ("notes", "notes: restored 2 items into bank-a"),
    ] {
        assert!(text.contains(shown), "{key} missing from text: {text}");
    }
    assert_eq!(
        text,
        "import: created 2, skipped 1, rejected 1, overwrote 1, \
             skipped invalid 1: itm-x#bank-a (out_of_range: too long)\n\
         written: bank-a/itm-1, bank-a/itm-2\n\
         dry_run: false\n\
         applied: true\n\
         created_ids: itm-1, itm-2\n\
         rejection_samples: rejected: category not accepted (profile)\n\
         notes: restored 2 items into bank-a"
    );
}

/// A carried-but-empty detail field is named `none`, not dropped; a field the
/// payload does not carry adds no line.
#[test]
fn an_empty_detail_field_is_named_not_dropped() {
    let text = render(&json!({"ok": true, "written": [],
        "report": {"dry_run": true, "would_create": 0, "would_skip": 0, "would_reject": 0,
                   "created_ids": [], "rejection_samples": [], "notes": []}}));
    assert_eq!(
        text,
        "import dry-run: would create 0, skip 0, reject 0 (zero writes)\n\
         written: none\n\
         dry_run: true\n\
         created_ids: none\n\
         rejection_samples: none\n\
         notes: none"
    );
}

/// A report that carries none of the list detail fields keeps the summary
/// line plus the run-state flags, so the older payloads' text view gains no
/// list line.
#[test]
fn a_report_without_list_detail_fields_keeps_the_summary_line() {
    let text = render(
        &json!({"ok": true, "report": {"dry_run": false, "applied": true,
                 "would_create": 2, "would_skip": 1, "would_reject": 0, "would_overwrite": 4}}),
    );
    assert_eq!(
        text,
        "import: created 2, skipped 1, rejected 0, overwrote 4\n\
         dry_run: false\n\
         applied: true"
    );
}
