// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the installation marker: the facts it records, the payload
//! digest that excludes it, the refusal of bytes it did not write, and the
//! canonical byte form two builds of the same assets produce.
//!
//! ## Owns
//! - Every assertion over `SkillMarker`, `payload_digest`, `owned_files`, and
//!   `sha256_hex`
//!
//! ## Does not own
//! - The marker implementation under test (`skill_marker`)
//! - The decision an installation makes from a marker (`skill_install_plan`)
//!
//! ## Boundary
//!
//! Test-only. Every fixture is built in memory from a fixed asset set, so no
//! assertion depends on a host path, a clock, or the real embedded payload's
//! bytes.

use super::*;

/// A fixed asset set with one nested name, so ordering and nesting are both
/// exercised without touching the real payload.
fn assets() -> Vec<(&'static str, &'static str)> {
    vec![
        ("SKILL.md", "# clio-mcp\n"),
        ("references/01-retrieval.md", "retrieval\n"),
        ("references/02-write.md", "write\n"),
    ]
}

const CATALOG: &str = "fnv1a64:0123456789abcdef";

#[test]
fn a_marker_round_trips_through_its_bytes() {
    let marker = SkillMarker::from_assets(&assets(), CATALOG);
    let bytes = marker.to_bytes().expect("the marker serializes");
    assert_eq!(SkillMarker::from_bytes(&bytes), Ok(marker));
}

#[test]
fn the_recorded_facts_name_the_installer_the_version_and_every_file() {
    let marker = SkillMarker::from_assets(&assets(), CATALOG);
    assert_eq!(marker.installer, "clio");
    assert_eq!(marker.package_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(marker.catalog_hash, CATALOG);
    let paths: Vec<&str> = marker.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        vec![
            "SKILL.md",
            "references/01-retrieval.md",
            "references/02-write.md"
        ],
        "the owned set is sorted by relative path"
    );
    assert_eq!(marker.payload_digest.len(), 64);
    for (path, hash) in &marker.files {
        assert_eq!(hash.len(), 64, "{path} is recorded as a SHA-256 in hex");
    }
}

#[test]
fn the_digest_ignores_the_marker_file() {
    let mut with_marker = assets();
    let without_marker = payload_digest(&with_marker);
    with_marker.push((SKILL_MARKER_FILE, "{\"installer\":\"clio\"}"));
    assert_eq!(payload_digest(&with_marker), without_marker);
    assert!(!owned_files(&with_marker).contains_key(SKILL_MARKER_FILE));
}

#[test]
fn the_digest_changes_when_one_asset_byte_changes() {
    let before = payload_digest(&assets());
    let mut changed = assets();
    changed[2].1 = "write!\n";
    assert_ne!(payload_digest(&changed), before);
    assert_ne!(owned_files(&changed), owned_files(&assets()));
}

#[test]
fn the_digest_changes_when_an_asset_is_renamed() {
    let mut renamed = assets();
    renamed[0].0 = "SKILL.markdown";
    assert_ne!(payload_digest(&renamed), payload_digest(&assets()));
}

#[test]
fn two_builds_of_the_same_assets_serialize_identically() {
    let first = SkillMarker::from_assets(&assets(), CATALOG)
        .to_bytes()
        .expect("the marker serializes");
    let second = SkillMarker::from_assets(&assets(), CATALOG)
        .to_bytes()
        .expect("the marker serializes");
    assert_eq!(first, second);
    let text = String::from_utf8(first).expect("the marker is UTF-8");
    let fields = [
        "\"installer\"",
        "\"package_version\"",
        "\"catalog_hash\"",
        "\"payload_digest\"",
        "\"files\"",
    ]
    .map(|field| text.find(field).expect("the field is recorded"));
    assert!(
        fields.windows(2).all(|pair| pair[0] < pair[1]),
        "the fields keep their declared order: {text}"
    );
    let retrieval = text
        .find("references/01-retrieval.md")
        .expect("the first reference is recorded");
    let write = text
        .find("references/02-write.md")
        .expect("the second reference is recorded");
    assert!(retrieval < write, "the files are sorted: {text}");
}

#[test]
fn bytes_that_are_not_a_marker_are_refused() {
    assert!(matches!(
        SkillMarker::from_bytes(b"not json"),
        Err(MarkerError::Malformed { .. })
    ));
    assert!(matches!(
        SkillMarker::from_bytes(b"{}"),
        Err(MarkerError::Malformed { .. })
    ));
}

#[test]
fn a_field_this_version_does_not_know_is_refused() {
    let marker = SkillMarker::from_assets(&assets(), CATALOG);
    let mut value = serde_json::to_value(&marker).expect("the marker is a JSON value");
    value["unknown"] = serde_json::json!(1);
    let bytes = serde_json::to_vec(&value).expect("the value serializes");
    assert!(matches!(
        SkillMarker::from_bytes(&bytes),
        Err(MarkerError::Malformed { .. })
    ));
}

#[test]
fn the_catalog_hash_is_the_one_the_schema_pack_publishes() {
    let expected = clio_mcp::schema::schema_pack()
        .get("catalog_hash")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .expect("the pack publishes a catalog hash");
    assert_eq!(catalog_hash(), expected);
    assert!(expected.starts_with("fnv1a64:"), "{expected}");
}

#[test]
fn the_embedded_assets_are_recorded_as_the_owned_set() {
    let marker = SkillMarker::from_assets(clio_mcp::skill_assets::SKILL_ASSETS, &catalog_hash());
    assert_eq!(
        marker.files.len(),
        clio_mcp::skill_assets::SKILL_ASSETS.len()
    );
    assert!(marker.files.contains_key("SKILL.md"));
    assert!(!marker.files.contains_key(SKILL_MARKER_FILE));
}
