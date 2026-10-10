// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module pins the reference `store` call used when migrating records in
//! from an external system, and checks that it stays inside the native
//! contract: a whitelisted `category`, a native `epistemic_kind`, a `context`
//! within the shared bound, and a snapshot that passes span verification
//! against its own `source_text`.
//!
//! ## Owns
//! - The contract check over the pinned migration example
//!
//! ## Does not own
//! - The admission, context, and span-verification code it exercises
//! - The CLI flag specs or MCP tool schemas
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! The example lives here as a constant rather than in a documentation page.
//! It used to be parsed out of a markdown guide at run time, which meant the
//! check only ran while that file was present: deleting the page would have
//! deleted the guarantee with it. A constant cannot go missing, and it puts the
//! example next to the assertions made about it.
//!
//! Change this example only when the migration contract itself changes, and
//! change the assertions with it rather than around it.

use clio_types::{EpistemicKind, SemanticCategory, validate_context};
use serde_json::Value;

/// The reference call for importing an external record.
///
/// `evidence_ref` carries the external identity opaquely: Clio does not open,
/// upsert, or fetch anything behind it. `context` is descriptive metadata only.
const MIGRATION_EXAMPLE: &str = r#"{
  "tool": "store",
  "args": {
    "item": {
      "id": "itm-migrated-42",
      "subject_id": "subject-9",
      "gist": "User runs a weekly release train on Tuesdays",
      "snapshot": {"entity": "release train", "date": "2024-04-02"}
    },
    "category": "task_spec",
    "epistemic_kind": "fact",
    "source_text": "The weekly release train runs on Tuesdays; the next run is 2024-04-02 (sprint notes).",
    "context": "imported from ticketing-export-2024, sprint notes section",
    "evidence_ref": "tickets:ACME-9182",
    "bank": "repo:/work/app"
  }
}"#;

/// Collect every string value stored under `key` anywhere in the JSON tree.
fn collect_strings(value: &Value, key: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (name, item) in map {
                if name == key
                    && let Some(text) = item.as_str()
                {
                    out.push(text.to_owned());
                }
                collect_strings(item, key, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_strings(item, key, out);
            }
        }
        _ => {}
    }
}

#[test]
fn the_migration_example_stays_inside_the_native_contract() {
    let example: Value =
        serde_json::from_str(MIGRATION_EXAMPLE).expect("the migration example must parse");

    let mut categories = Vec::new();
    collect_strings(&example, "category", &mut categories);
    assert!(
        !categories.is_empty(),
        "the migration example must exercise a category gate"
    );
    for category in &categories {
        SemanticCategory::parse(category).unwrap_or_else(|error| {
            panic!("migration example category `{category}` is not whitelisted: {error}")
        });
    }

    let mut kinds = Vec::new();
    collect_strings(&example, "epistemic_kind", &mut kinds);
    for kind in &kinds {
        EpistemicKind::parse(kind).unwrap_or_else(|error| {
            panic!("migration example epistemic_kind `{kind}` is not native: {error}")
        });
    }

    let mut contexts = Vec::new();
    collect_strings(&example, "context", &mut contexts);
    for context in &contexts {
        validate_context(context).unwrap_or_else(|error| {
            panic!("migration example context value violates the native bound: {error}")
        });
    }

    // The example must be executable, not merely well-shaped: its snapshot has
    // to pass native span verification against its own source_text. A
    // shape-only check lets a non-verifiable example ship.
    let args = example.get("args").expect("migration example has args");
    let snapshot = args
        .pointer("/item/snapshot")
        .expect("migration example pins a snapshot");
    let source_text = args
        .get("source_text")
        .and_then(Value::as_str)
        .expect("migration example pins the source_text haystack");
    let report = clio_write::verify::verify_snapshot(
        source_text,
        snapshot,
        &clio_write::schema::default_event_fields(),
        &clio_write::verify::VerifyConfig::default(),
    );
    assert!(
        report.ok,
        "migration example snapshot must pass native span verification: {:?}",
        report.failures
    );
}
