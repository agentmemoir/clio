// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the committed catalog hash in the skill package to the hash this build publishes, so the package can never describe a catalog the build no longer publishes.
//!
//! ## Owns
//! - The committed hash at `skills/clio-mcp/catalog_hash.txt`
//! - The canonical one-line rendering every comparison uses
//! - The ignored regenerator that rewrites the committed file
//!
//! ## Does not own
//! - The catalog hash itself (`clio_mcp::schema`)
//! - The skill prose or its reference files
//! - The schema pack fixture, which pins the whole pack instead
//!
//! ## Boundary
//!
//! A change to a published tool's name, description, or schema should normally require the committed hash to be regenerated here. A change to how the hash is computed should not: this module compares the rendered value, not its input.
//!
//! The committed text is embedded with `include_str!`, so the check opens no path and reads no clock, working directory, or environment. The only writer is the ignored regenerator, so an ordinary run cannot mask a stale file by rewriting it.

use std::fs;

use clio_mcp::schema::{catalog_hash, schema_pack};
use serde_json::Value;

/// The committed hash file, resolved from the crate root so the regenerator
/// writes the same path on every machine.
const HASH_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../skills/clio-mcp/catalog_hash.txt"
);

/// The committed package text, embedded at compile time.
const COMMITTED: &str = include_str!("../../../../skills/clio-mcp/catalog_hash.txt");

/// The version prefix every rendered hash carries.
const PREFIX: &str = "fnv1a64:";

/// The repair command every failure message names.
const REPAIR: &str = "cargo test -p clio-mcp --test skill_drift -- --ignored --exact catalog_hash::regenerate_catalog_hash";

/// The reason the committed text does not match the current catalog hash.
#[derive(Debug, PartialEq, Eq)]
enum Drift {
    /// The text is not one rendered line with its trailing newline.
    Malformed(String),
    /// The text is well formed and names a different catalog hash.
    Stale { committed: String, current: String },
}

impl Drift {
    /// The failure text for this drift, naming the rule and the repair.
    fn message(&self) -> String {
        match self {
            Self::Malformed(text) => format!(
                "the skill package catalog hash is malformed: `{text}` is not `{PREFIX}` plus sixteen lowercase hex digits followed by exactly one newline, so it cannot match the current catalog hash; the repair is to run `{REPAIR}` and commit the result",
                text = text.trim_end_matches('\n')
            ),
            Self::Stale { committed, current } => format!(
                "the skill package carries the committed hash `{committed}` against the current catalog hash `{current}`, so it is stale; the fix is to run `{REPAIR}` and commit the result, because a stale hash lets the package describe a catalog this build no longer publishes"
            ),
        }
    }
}

/// The canonical committed text for `hash`: the hash on one line with one
/// trailing newline.
fn render(hash: &str) -> String {
    format!("{hash}\n")
}

/// The catalog hash the published pack carries.
fn current_hash() -> String {
    schema_pack()["catalog_hash"]
        .as_str()
        .expect("the published schema pack carries catalog_hash as a string")
        .to_owned()
}

/// The tools the published pack carries, in catalog order.
fn pack_tools() -> Vec<Value> {
    schema_pack()["tools"]
        .as_array()
        .expect("the published schema pack carries a tools[] array")
        .clone()
}

/// Whether `text` is exactly one rendered line: the version prefix, sixteen
/// lowercase hex digits, and one trailing newline.
fn is_well_formed(text: &str) -> bool {
    let Some(line) = text.strip_suffix('\n') else {
        return false;
    };
    if line.ends_with('\n') {
        return false;
    }
    let Some(digits) = line.strip_prefix(PREFIX) else {
        return false;
    };
    digits.len() == 16
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Compare the committed `text` with `current`, rejecting a malformed line
/// before comparing, so a broken file never reads as merely stale.
fn classify(text: &str, current: &str) -> Result<(), Drift> {
    if !is_well_formed(text) {
        return Err(Drift::Malformed(text.to_owned()));
    }
    if text != render(current) {
        return Err(Drift::Stale {
            committed: text.trim_end_matches('\n').to_owned(),
            current: current.to_owned(),
        });
    }
    Ok(())
}

/// Apply `mutate` to the catalog tools, prove the committed text rejects the
/// result and a fresh render accepts it, then prove `revert` restores the
/// original hash and the committed text accepts that again.
fn assert_mutation_is_detected(
    label: &str,
    mutate: impl Fn(&mut Vec<Value>),
    revert: impl Fn(&mut Vec<Value>),
) {
    let mut tools = pack_tools();
    let original = catalog_hash(&tools);
    let committed = render(&original);
    assert!(
        classify(&committed, &original).is_ok(),
        "{label}: unmutated"
    );
    mutate(&mut tools);
    let mutated = catalog_hash(&tools);
    assert_ne!(mutated, original, "{label}: changes the catalog hash");
    assert!(
        matches!(classify(&committed, &mutated), Err(Drift::Stale { .. })),
        "{label}: the mutated catalog is stale"
    );
    assert!(
        classify(&render(&mutated), &mutated).is_ok(),
        "{label}: regenerated"
    );
    revert(&mut tools);
    assert_eq!(catalog_hash(&tools), original, "{label}: revert restores");
    assert!(
        classify(&committed, &original).is_ok(),
        "{label}: passes again"
    );
}

/// Protects that the package the build ships carries the hash this build
/// publishes, so a tool change cannot land without the package changing with
/// it.
#[test]
fn the_package_carries_the_regenerated_catalog_hash() {
    let current = current_hash();
    if let Err(drift) = classify(COMMITTED, &current) {
        panic!("{}", drift.message());
    }
}

/// Protects that an old hash value is rejected with the repair command, so a
/// stale package cannot pass a check by looking like a valid line.
#[test]
fn a_stale_catalog_hash_is_rejected() {
    let current = current_hash();
    let old = "fnv1a64:0000000000000000";
    assert_ne!(old, current, "the fixed old value is not the current hash");
    let drift = classify(&render(old), &current).expect_err("an old hash is stale");
    assert!(
        matches!(drift, Drift::Stale { .. }),
        "the old hash is stale"
    );
    let message = drift.message();
    assert!(message.contains("stale"), "the message names staleness");
    assert!(
        message.contains(REPAIR),
        "the message names the repair command"
    );
    assert!(
        message.contains("describe a catalog this build no longer publishes"),
        "the message names why a stale hash is unsafe"
    );
}

/// Protects that a line which is not one rendered hash is rejected as
/// malformed, not reported as a correct hash that merely needs a rerun.
#[test]
fn a_malformed_catalog_hash_is_rejected() {
    let current = current_hash();
    let digits = current.trim_start_matches(PREFIX);
    let short = "0".repeat(15);
    let cases = [
        render(digits),
        render(&format!("{PREFIX}{short}")),
        render(&format!("{PREFIX}ABCDEF0123456789")),
        digits.to_owned(),
    ];
    for text in cases {
        let drift = classify(&text, &current).expect_err("a malformed line cannot match");
        assert!(
            matches!(drift, Drift::Malformed(_)),
            "{text:?} is malformed"
        );
        let message = drift.message();
        assert!(
            message.contains("malformed"),
            "the message names malformation"
        );
        assert!(!message.contains("stale"), "a malformed line is not stale");
    }
}

/// Protects that every kind of catalog edit makes the committed hash stale,
/// that regenerating from the mutated catalog produces a passing line, and
/// that undoing the edit restores the original hash.
#[test]
fn each_catalog_mutation_makes_the_committed_hash_stale_and_regeneration_passes() {
    let tools = pack_tools();
    assert!(
        tools.len() >= 2,
        "the catalog publishes at least two tools to mutate"
    );
    let first_name = tools[0]["name"].clone();
    let first_description = tools[0]["description"].clone();
    let last = tools.last().expect("the catalog publishes tools").clone();
    let mut appended = last.clone();
    appended["name"] = serde_json::json!("__appended_catalog_hash_probe__");

    assert_mutation_is_detected(
        "rename",
        |tools| tools[0]["name"] = serde_json::json!("__renamed_catalog_hash_probe__"),
        |tools| tools[0]["name"] = first_name.clone(),
    );
    assert_mutation_is_detected(
        "add",
        |tools| tools.push(appended.clone()),
        |tools| {
            tools.pop().expect("the appended tool");
        },
    );
    assert_mutation_is_detected(
        "remove",
        |tools| {
            tools.pop().expect("the last tool");
        },
        |tools| tools.push(last.clone()),
    );
    assert_mutation_is_detected(
        "description",
        |tools| {
            tools[0]["description"] =
                serde_json::json!("a description changed by the catalog hash check");
        },
        |tools| tools[0]["description"] = first_description.clone(),
    );
    assert_mutation_is_detected(
        "inputSchema property",
        |tools| {
            tools[0]["inputSchema"]["properties"]["__hash_probe"] =
                serde_json::json!({"type": "string"});
        },
        |tools| {
            tools[0]["inputSchema"]["properties"]
                .as_object_mut()
                .expect("inputSchema.properties is an object")
                .remove("__hash_probe");
        },
    );
}

/// Protects that rendering the same hash twice is byte-identical and ends with
/// exactly one newline, so the regenerator cannot produce a diff from nothing.
#[test]
fn regeneration_is_byte_identical_across_runs() {
    let current = current_hash();
    let first = render(&current);
    let second = render(&current);
    assert_eq!(
        first, second,
        "rendering the same hash twice is byte-identical"
    );
    assert!(first.ends_with('\n'), "the render ends with a newline");
    assert_eq!(
        first.matches('\n').count(),
        1,
        "the render ends with exactly one newline"
    );
}

/// Rewrite the committed hash with the current catalog hash. Ignored because
/// it is the only writer of the package file.
#[test]
#[ignore = "writes skills/clio-mcp/catalog_hash.txt"]
fn regenerate_catalog_hash() {
    let text = render(&current_hash());
    fs::write(HASH_PATH, &text).expect("write skills/clio-mcp/catalog_hash.txt");
    assert_eq!(
        fs::read_to_string(HASH_PATH).expect("read skills/clio-mcp/catalog_hash.txt"),
        text,
        "the regenerated file holds exactly the rendered current hash"
    );
}
