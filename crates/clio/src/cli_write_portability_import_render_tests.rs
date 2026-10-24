// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `import` human text view: a live run that restored
//! nothing says so with the skipped count, and a run that skipped an entity as
//! invalid names it with its code and reason instead of hiding it.
//!
//! ## Owns
//! - The live zero-create, zero-overwrite render line of `cli_write_portability`
//! - The skipped-as-invalid tail of the live render lines
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

/// A live run that created and overwrote nothing says nothing was restored.
#[test]
fn render_says_a_live_zero_write_import_restored_nothing() {
    let group = super::PortabilityWriteGroup;
    let line = group.render(
        "import",
        &json!({"ok": true, "report": {"dry_run": false, "applied": true, "would_create": 0,
                 "would_skip": 4840, "would_reject": 0, "would_overwrite": 0}}),
        crate::cli_style::Palette::new(false),
    );
    assert_eq!(
        line,
        "import: nothing restored: created 0, overwrote 0, skipped 4840, rejected 0"
    );
}

/// A live run that skipped one entity as invalid names it, with code and reason.
#[test]
fn render_names_the_row_a_live_import_skipped_as_invalid() {
    let group = super::PortabilityWriteGroup;
    let line = group.render(
        "import",
        &json!({"ok": true, "report": {"dry_run": false, "applied": true, "would_create": 1,
                 "would_skip": 0, "would_reject": 0, "would_overwrite": 0,
                 "would_skip_invalid": 1,
                 "skipped_invalid": [{"id": "itm-legacy#bank-a", "code": "out_of_range",
                                      "reason": "gist exceeds 250 characters (1079)"}]}}),
        crate::cli_style::Palette::new(false),
    );
    assert_eq!(
        line,
        "import: created 1, skipped 0, rejected 0, overwrote 0, skipped invalid 1: \
         itm-legacy#bank-a (out_of_range: gist exceeds 250 characters (1079))"
    );
}

/// An all-invalid run names the count, never "skipped 0", and caps the list.
#[test]
fn render_of_an_all_invalid_import_counts_every_skipped_row() {
    let group = super::PortabilityWriteGroup;
    let line = group.render(
        "import",
        &json!({"ok": true, "report": {"dry_run": false, "applied": true, "would_create": 0,
                 "would_skip": 0, "would_reject": 0, "would_overwrite": 0,
                 "would_skip_invalid": 5,
                 "skipped_invalid": [
                     {"id": "itm-1#bank-a", "code": "out_of_range", "reason": "too long"},
                     {"id": "itm-2#bank-a", "code": "out_of_range", "reason": "too long"},
                     {"id": "itm-3#bank-a", "code": "out_of_range", "reason": "too long"}]}}),
        crate::cli_style::Palette::new(false),
    );
    assert_eq!(
        line,
        "import: nothing restored: created 0, overwrote 0, skipped 0, rejected 0, \
         skipped invalid 5: itm-1#bank-a (out_of_range: too long); \
         itm-2#bank-a (out_of_range: too long); itm-3#bank-a (out_of_range: too long) \
         and 2 more"
    );
}
