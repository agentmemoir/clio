// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the committed pack-shape record to the
//! pack this crate actually builds, so the record that says why one shape ships
//! cannot drift away from the shape the build emits.
//!
//! ## Owns
//! - `the_record_ships_the_shape_the_crate_emits`
//! - `the_record_shipped_entry_is_the_shipped_candidate`
//! - `the_record_shipped_digest_is_the_shipped_pack_digest`
//! - `the_record_holds_both_candidate_measurements`
//! - `the_record_probe_status_is_allowed`
//!
//! ## Does not own
//! - The pack document itself, or which shape ships (`clio-mcp` `schema` and
//!   `schema_shape`)
//! - Producing the record, its probe receipt, or its measurements (the
//!   benchmark directory owns those)
//!
//! ## Boundary
//!
//! A change to which shape ships, or to the shipped pack, should normally
//! require the record to be regenerated beside it. A change to how a
//! measurement is taken should not: this test compares the recorded digest with
//! the digest of the bytes the crate emits, both canonicalized the same way.
//!
//! The record is read from the checkout rather than embedded, because it is a
//! measurement record and not shipped data: the two paths below resolve from
//! the crate root, and nothing here reads the clock, the network, or the
//! environment.

use std::path::{Path, PathBuf};

use clio_compliance::sha256_hex_bytes;
use clio_mcp::schema::schema_pack;
use clio_mcp::schema_shape::{PackShape, SHIPPED_PACK_SHAPE};
use serde_json::Value;

/// The one shape record, relative to the repository root.
const RECORD_RELATIVE: &str = "benchmarks/mcp_token_budget/pack_shape.json";

/// The shipped pack fixture the record's shipped digest is taken over.
const SHIPPED_RELATIVE: &str = "crates/clio-mcp/tests/fixtures/schema_pack.json";

/// The three statuses a probe receipt may carry, and nothing else.
const STATUSES: [&str; 3] = ["not_run", "passed", "failed"];

/// The repository root, one crate directory above two levels.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The JSON object committed at `relative` under the repository root.
fn read_json(relative: &str) -> Value {
    let path = repo_root().join(relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()))
}

/// The shape this crate ships, spelled the way the record spells it.
fn shipped_shape_name() -> &'static str {
    match SHIPPED_PACK_SHAPE {
        PackShape::Reference => "reference",
        PackShape::Flattened => "flattened",
    }
}

/// The canonical bytes the budget gate measures: the tools array with sorted
/// keys and no insignificant whitespace.
fn canonical_tools(pack: &Value) -> String {
    let tools = pack
        .get("tools")
        .and_then(Value::as_array)
        .expect("a pack document holds a tools array");
    serde_json::to_string(tools).expect("a tools array serializes")
}

#[test]
fn the_record_ships_the_shape_the_crate_emits() {
    let record = read_json(RECORD_RELATIVE);
    let shipped = record["shipped"]["shape"]
        .as_str()
        .expect("the record names its shipped shape");
    assert_eq!(
        shipped,
        shipped_shape_name(),
        "the record names the emitted shape"
    );
}

#[test]
fn the_record_shipped_entry_is_the_shipped_candidate() {
    let record = read_json(RECORD_RELATIVE);
    let shipped = &record["shipped"];
    assert_eq!(
        shipped["shape"].as_str(),
        Some(shipped_shape_name()),
        "the shipped entry names the shape this crate ships"
    );
    assert_eq!(
        shipped["path"].as_str(),
        Some(SHIPPED_RELATIVE),
        "the shipped entry points at the shipped fixture"
    );

    let fixture_digest = sha256_hex_bytes(canonical_tools(&read_json(SHIPPED_RELATIVE)).as_bytes());
    assert_eq!(
        shipped["sha256"].as_str(),
        Some(fixture_digest.as_str()),
        "the shipped entry's digest is the shipped fixture's digest"
    );

    let candidates = record["candidates"]
        .as_array()
        .expect("the record holds a candidates array");
    let matching: Vec<&Value> = candidates
        .iter()
        .filter(|candidate| candidate["sha256"].as_str() == Some(fixture_digest.as_str()))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one candidate entry carries the shipped fixture's digest"
    );
    assert_eq!(
        matching[0]["path"].as_str(),
        Some(SHIPPED_RELATIVE),
        "the entry that matches the fixture is the shipped fixture's entry"
    );
    assert_eq!(
        matching[0]["shape"].as_str(),
        Some(shipped_shape_name()),
        "the entry that matches the fixture names the shape this crate ships"
    );
}

#[test]
fn the_record_shipped_digest_is_the_shipped_pack_digest() {
    let record = read_json(RECORD_RELATIVE);
    let recorded = record["shipped"]["sha256"]
        .as_str()
        .expect("the record names its shipped digest");

    let fixture = read_json(SHIPPED_RELATIVE);
    let canonical = canonical_tools(&fixture);
    assert_eq!(
        recorded,
        sha256_hex_bytes(canonical.as_bytes()),
        "the record's shipped digest is the shipped fixture's digest"
    );

    let built = canonical_tools(&schema_pack());
    assert_eq!(
        recorded,
        sha256_hex_bytes(built.as_bytes()),
        "the record's shipped digest is the digest of the pack the crate builds"
    );
}

#[test]
fn the_record_holds_both_candidate_measurements() {
    let record = read_json(RECORD_RELATIVE);
    let candidates = record["candidates"]
        .as_array()
        .expect("the record holds a candidates array");
    assert_eq!(candidates.len(), 2, "both pack shapes are measured");
    let shapes: Vec<&str> = candidates
        .iter()
        .map(|candidate| {
            candidate["shape"]
                .as_str()
                .expect("each candidate names its shape")
        })
        .collect();
    assert!(
        shapes.contains(&"reference"),
        "the reference candidate is measured"
    );
    assert!(
        shapes.contains(&"flattened"),
        "the flattened candidate is measured"
    );

    for candidate in candidates {
        let path = candidate["path"]
            .as_str()
            .expect("each candidate names its path");
        assert!(
            !path.starts_with('/') && !path.contains(".."),
            "a candidate path is repo-relative: {path}"
        );
        let pack = read_json(path);
        let canonical = canonical_tools(&pack);
        assert!(
            usize::try_from(candidate["bytes"].as_u64().expect("bytes is a number"))
                .expect("the recorded bytes fit a usize")
                == canonical.len(),
            "{path}: the recorded bytes are the canonical tools byte length"
        );
        assert!(
            candidate["tokens"].as_u64().expect("tokens is a number") > 0,
            "{path}: the recorded token count is positive"
        );
        assert_eq!(
            candidate["sha256"].as_str().expect("sha256 is a string"),
            sha256_hex_bytes(canonical.as_bytes()),
            "{path}: the recorded digest is the file's canonical tools digest"
        );
    }
}

#[test]
fn the_record_probe_status_is_allowed() {
    let record = read_json(RECORD_RELATIVE);
    let status = record["probe"]["status"]
        .as_str()
        .expect("the record carries a probe status");
    assert!(
        STATUSES.contains(&status),
        "the probe status is one of {STATUSES:?}: {status}"
    );
    if status != "passed" {
        assert_eq!(
            shipped_shape_name(),
            "flattened",
            "a probe that did not pass ships the flattened pack"
        );
        assert_eq!(
            record["shipped"]["shape"].as_str(),
            Some("flattened"),
            "the record says the flattened pack ships"
        );
    }
}
