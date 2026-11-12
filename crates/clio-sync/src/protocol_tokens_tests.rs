// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the opaque sync tokens: the cursor and watermark codecs,
//! their checksums, and the acceptance of a token written by an older build
//! under the legacy v1 tag.
//!
//! ## Owns
//! - Codec round trips, malformed shapes, and v1 acceptance for both tokens
//!
//! ## Does not own
//! - The rest of the wire contract (`protocol_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a token body or checksum changes; they never change
//! either.

use super::{decode_cursor, decode_watermark, encode_cursor, encode_watermark};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// A cursor token minted by an older build: the v1 tag prefix and no v2 tag.
fn legacy_cursor(bank: &str, seq: i64) -> String {
    let preimage = format!("{bank}|{seq}");
    let tag = crate::legacy_tags::legacy_sha256_hex(crate::legacy_tags::SYNC_CURSOR, &preimage)
        .expect("legacy tag data");
    URL_SAFE_NO_PAD.encode(format!("{seq}:{}", &tag[..12]))
}

/// A watermark token minted by an older build: the v1 tag prefix.
fn legacy_watermark(bank: &str, stamp: &str) -> String {
    let preimage = format!("{bank}|{stamp}");
    let tag = crate::legacy_tags::legacy_sha256_hex(crate::legacy_tags::SYNC_WATERMARK, &preimage)
        .expect("legacy tag data");
    URL_SAFE_NO_PAD.encode(format!("wm:{stamp}:{}", &tag[..12]))
}

#[test]
fn cursor_roundtrips_and_refuses_another_bank() {
    let token = encode_cursor("bank-a", 42);
    assert_eq!(decode_cursor(&token, "bank-a").expect("roundtrip"), 42);
    let err = decode_cursor(&token, "bank-b").expect_err("checksum binds the bank");
    assert!(err.message.contains("checksum mismatch"));
}

#[test]
fn cursor_accepts_the_legacy_v1_tag() {
    assert_eq!(
        decode_cursor(&legacy_cursor("bank-a", 7), "bank-a").expect("legacy token"),
        7
    );
}

#[test]
fn cursor_refuses_every_malformed_shape() {
    assert!(decode_cursor("!!!", "bank-a").is_err(), "not base64");
    let no_separator = URL_SAFE_NO_PAD.encode("42");
    assert!(
        decode_cursor(&no_separator, "bank-a").is_err(),
        "no separator"
    );
    let not_a_number = URL_SAFE_NO_PAD.encode("x:abcdefabcdef");
    assert!(
        decode_cursor(&not_a_number, "bank-a").is_err(),
        "sequence is not a number"
    );
    let not_utf8 = URL_SAFE_NO_PAD.encode([0xff, 0xfe]);
    assert!(decode_cursor(&not_utf8, "bank-a").is_err(), "not UTF-8");
}

#[test]
fn watermark_roundtrips_and_accepts_the_legacy_v1_tag() {
    let stamp = "2026-01-01T00:00:00Z";
    let token = encode_watermark("bank-a", stamp);
    assert_eq!(
        decode_watermark("bank-a", &token).expect("roundtrip"),
        stamp
    );
    assert_eq!(
        decode_watermark("bank-a", &legacy_watermark("bank-a", stamp)).expect("legacy token"),
        stamp
    );
    assert!(
        decode_watermark("bank-b", &token).is_err(),
        "checksum binds the bank"
    );
}

#[test]
fn watermark_refuses_every_malformed_shape() {
    assert_eq!(
        decode_watermark("bank-a", "").expect("empty token is the epoch"),
        "1970-01-01T00:00:00Z"
    );
    assert!(decode_watermark("bank-a", "!!!").is_err(), "not base64");
    let no_prefix = URL_SAFE_NO_PAD.encode("2026-01-01T00:00:00Z:abcdefabcdef");
    assert!(
        decode_watermark("bank-a", &no_prefix).is_err(),
        "no wm: prefix"
    );
    let no_separator = URL_SAFE_NO_PAD.encode("wm:2026-01-01T00:00:00Z");
    assert!(
        decode_watermark("bank-a", &no_separator).is_err(),
        "no checksum"
    );
    let not_utf8 = URL_SAFE_NO_PAD.encode([0xff, 0xfe]);
    assert!(decode_watermark("bank-a", &not_utf8).is_err(), "not UTF-8");
}
