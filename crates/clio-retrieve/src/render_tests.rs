// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the cap-boundary, determinism, and
//! whole-value-only behaviour of `render_exact`.
//!
//! ## Owns
//! - The exact byte-boundary assertions at, just under, and just over the cap
//! - The proof that an over-cap snapshot yields nothing rather than a prefix
//! - The determinism assertion for one fixed snapshot value
//! - The byte-not-char accounting for multi-byte UTF-8 content
//! - The token upper bound the byte cap implies, and a cap-filling rendering
//!   of multi-byte characters that reaches it
//!
//! ## Does not own
//! - Whether a request asked for a rendering (`finalize_tests` covers that)
//! - The hit field's serde shape (`clio-types` owns `RetrieveHit`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Fixtures build a snapshot whose serialized length is computed, then padded to
//! hit an exact byte count (`fixtures_tests::snapshot_of_bytes`, shared with the
//! hit-assembly tests), so a change to the cap fails the boundary test with the
//! real length in the message instead of silently drifting. No clock,
//! environment variable, port, path, or random source is read here, so every test
//! is order-independent and repeatable.

use super::{EXACT_RENDER_MAX_BYTES, render_exact};
use crate::fixtures_tests as fx;
use crate::token::estimate_tokens;
use serde_json::Value;
use serde_json::json;

#[test]
fn a_rendering_of_exactly_the_cap_is_present() {
    assert_eq!(EXACT_RENDER_MAX_BYTES, 512, "the published cap value");
    let snapshot = fx::snapshot_of_bytes(EXACT_RENDER_MAX_BYTES);
    let rendered = render_exact(&snapshot).expect("512 bytes is within the cap");
    assert_eq!(
        rendered.len(),
        EXACT_RENDER_MAX_BYTES,
        "the whole rendering is returned, not a part of it"
    );
    assert_eq!(
        rendered,
        serde_json::to_string(&snapshot).expect("serialize"),
        "the rendering is the snapshot's own serialization"
    );
}

#[test]
fn a_rendering_one_byte_under_the_cap_is_present() {
    let snapshot = fx::snapshot_of_bytes(EXACT_RENDER_MAX_BYTES - 1);
    let rendered = render_exact(&snapshot).expect("511 bytes is within the cap");
    assert_eq!(rendered.len(), 511, "the full 511 bytes are returned");
}

#[test]
fn a_rendering_one_byte_over_the_cap_yields_nothing_and_never_a_prefix() {
    let snapshot = fx::snapshot_of_bytes(EXACT_RENDER_MAX_BYTES + 1);
    let full = serde_json::to_string(&snapshot).expect("serialize");
    assert_eq!(full.len(), 513, "the input is one byte over the cap");
    assert!(
        render_exact(&snapshot).is_none(),
        "an over-cap snapshot carries no rendering; a prefix would read as the \
         stored value (the 513-byte input was {full})"
    );
}

#[test]
fn a_snapshot_whose_rendering_is_far_over_the_cap_yields_nothing() {
    let snapshot = json!({ "filler": "x".repeat(EXACT_RENDER_MAX_BYTES * 4) });
    assert!(
        render_exact(&snapshot).is_none(),
        "an oversized snapshot is omitted whole, not shortened"
    );
}

#[test]
fn the_same_snapshot_renders_to_the_same_text() {
    let snapshot = json!({
        "unit_id": "u-1",
        "hub_id": "h-1",
        "neighbor_ids": ["n-1", "n-2", "n-3"],
        "neighbor_count": 3,
    });
    let first = render_exact(&snapshot).expect("within the cap");
    let second = render_exact(&snapshot).expect("within the cap");
    assert_eq!(first, second, "the same value renders to the same text");
    // A re-parsed copy is the same value, so it renders identically too.
    let reparsed: Value = serde_json::from_str(&first).expect("reparse");
    assert_eq!(
        render_exact(&reparsed).expect("within the cap"),
        first,
        "re-parsing the rendering and rendering it again is stable"
    );
}

#[test]
fn object_keys_render_in_sorted_order_not_insertion_order() {
    let forwards = json!({ "b": 2, "a": 1, "c": 3 });
    let backwards = json!({ "c": 3, "a": 1, "b": 2 });
    assert_eq!(
        render_exact(&forwards).expect("within the cap"),
        render_exact(&backwards).expect("within the cap"),
        "insertion order cannot change the rendering"
    );
    assert_eq!(
        render_exact(&forwards).expect("within the cap"),
        r#"{"a":1,"b":2,"c":3}"#,
        "keys serialize in sorted order"
    );
}

#[test]
fn non_object_snapshots_render_without_panicking() {
    assert_eq!(
        render_exact(&Value::Null).as_deref(),
        Some("null"),
        "a null snapshot is a two-character rendering"
    );
    assert_eq!(
        render_exact(&json!([1, 2, 3])).as_deref(),
        Some("[1,2,3]"),
        "an array snapshot renders whole"
    );
    assert_eq!(
        render_exact(&json!("text")).as_deref(),
        Some("\"text\""),
        "a string snapshot is rendered with its quotes"
    );
}

#[test]
fn an_empty_object_renders_within_the_cap() {
    assert_eq!(
        render_exact(&json!({})).as_deref(),
        Some("{}"),
        "a snapshot-less item still has a rendering"
    );
}

#[test]
fn multi_byte_content_is_counted_in_bytes_not_characters() {
    // 128 three-byte characters: 128 characters, 384 bytes of content, and a
    // rendering still inside the 512-byte cap even though its char count alone
    // would not decide the question.
    let snapshot = json!({ "filler": "\u{4e00}".repeat(128) });
    let rendered = render_exact(&snapshot).expect("384 content bytes fit the cap");
    assert_eq!(
        rendered.len(),
        397,
        "128 three-byte characters plus the wrapper"
    );
    assert_eq!(
        rendered.chars().count(),
        141,
        "the same characters, counted as chars"
    );
    assert!(
        rendered.len() <= EXACT_RENDER_MAX_BYTES,
        "rendering is {} bytes, inside the 512-byte cap",
        rendered.len()
    );

    // The same shape pushed past the cap in bytes is refused, which a
    // char-count check would have let through.
    let over = json!({ "filler": "\u{4e00}".repeat(200) });
    assert!(
        render_exact(&over).is_none(),
        "600 content bytes is over the 512-byte cap"
    );
}

/// The published token bound: a rendering of at most the cap's bytes costs at
/// most a quarter of that under `estimate_tokens`. The cap counts bytes and the
/// estimator counts characters, so the bound is a character count and not an
/// ASCII claim: the text below reaches it with multi-byte characters in it.
#[test]
fn a_rendering_within_the_byte_cap_costs_at_most_a_quarter_of_its_bytes_in_tokens() {
    let bound = EXACT_RENDER_MAX_BYTES / 4;
    let full = render_exact(&fx::snapshot_of_bytes(EXACT_RENDER_MAX_BYTES)).expect("512 bytes fit");
    assert_eq!(
        estimate_tokens(&full),
        bound,
        "a cap-filling rendering reaches the bound exactly"
    );

    for count in [1_usize, 64, 128, 160] {
        let snapshot = json!({ "filler": "\u{4e00}".repeat(count) });
        let rendered = render_exact(&snapshot).expect("three-byte characters still fit the cap");
        assert!(
            rendered.len() <= EXACT_RENDER_MAX_BYTES,
            "{} bytes is outside the {EXACT_RENDER_MAX_BYTES} byte cap",
            rendered.len()
        );
        assert!(
            estimate_tokens(&rendered) <= bound,
            "{} characters cost {} tokens, over the {bound} token bound",
            rendered.chars().count(),
            estimate_tokens(&rendered)
        );
    }
}

/// The bound is a character count, so multi-byte text reaches it exactly like
/// ASCII does: 495 ASCII characters and one four-byte character fill the cap's
/// 512 bytes with 509 characters, and 509 characters is still the bound. Without
/// this the "only ASCII reaches the bound" wording would look true, because the
/// arithmetic above only ever fills the cap with one-byte characters.
///
/// The character counts are pinned so a change in the estimator or in the cap
/// fails here with the real numbers instead of drifting silently.
#[test]
fn a_cap_filling_rendering_of_multi_byte_characters_costs_the_bound_too() {
    let bound = EXACT_RENDER_MAX_BYTES / 4;
    // The wrapper `{"filler":"` and its closing `"}` cost 13 characters, so 496
    // characters of content leave the rendering at 509 characters and, with one
    // four-byte character among them, at 512 bytes.
    let content = format!("{}\u{1f600}", "a".repeat(495));
    let snapshot = json!({ "filler": content });
    let rendered = render_exact(&snapshot).expect("512 bytes are within the cap");

    assert_eq!(
        rendered.len(),
        EXACT_RENDER_MAX_BYTES,
        "the rendering fills the cap in bytes"
    );
    assert_eq!(
        rendered.chars().count(),
        509,
        "three bytes more than the cap, so three characters fewer"
    );
    assert!(
        !rendered.is_ascii(),
        "the rendering holds a multi-byte character: {rendered}"
    );
    assert_eq!(
        estimate_tokens(&rendered),
        bound,
        "509 characters is the {bound} token bound, multi-byte characters included"
    );
}
