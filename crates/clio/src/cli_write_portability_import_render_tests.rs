// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `import` human text view for a live run that restored
//! nothing: the line says so and carries the skipped count, instead of reading
//! like a successful restore with a zero create count.
//!
//! ## Owns
//! - The live zero-create, zero-overwrite render line of `cli_write_portability`
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
