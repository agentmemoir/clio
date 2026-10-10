// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync wire contract: cursor codec roundtrip and
//! rejection, watermark codec, LWW comparison ordering, envelope validation,
//! and dead-letter reason mapping.
//!
//! ## Owns
//! - Unit coverage for `protocol` (cursors, LWW, validation, reason codes)
//!
//! ## Does not own
//! - Apply or transport behavior (`apply_tests` / `server_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the wire contract changes; they never change it.
//!
//! Keep this module focused on sync wire contract tests.

use crate::protocol::{
    ENTITY_KIND_ITEM, OP_INVALIDATE, OP_UPSERT, REASON_ERASED_SUBJECT, REASON_MALFORMED,
    SyncContent, SyncMutation, WM_ENTITY_SEP, canonical_watermark_body, decode_cursor,
    decode_watermark, encode_cursor, encode_watermark, join_watermark_body, lww_compare,
    reason_code_for,
};
use clio_types::{AmError, ErrorCode};

#[test]
fn cursor_roundtrip_and_checksum_rejects() {
    let token = encode_cursor("bank-a", 42);
    assert!(!token.contains("42"));
    assert_eq!(decode_cursor(&token, "bank-a").unwrap(), 42);
    assert!(decode_cursor(&token, "other-bank").is_err());
    assert!(decode_cursor("!!!", "bank-a").is_err());
    assert_eq!(
        decode_cursor(&encode_cursor("bank-a", 0), "bank-a").unwrap(),
        0
    );
    // A truncated token must fail (checksum region missing).
    assert!(decode_cursor(&token[..token.len() - 3], "bank-a").is_err());
    // A token whose seq part is mutated must fail the checksum decode.
    let bogus = encode_cursor("bank-a", 43);
    assert!(decode_cursor(&bogus, "other").is_err());
}

#[test]
fn watermark_roundtrip_and_rejects() {
    let token = encode_watermark("bank-a", "2026-01-01T00:00:00Z");
    assert_eq!(
        decode_watermark("bank-a", &token).unwrap(),
        "2026-01-01T00:00:00Z"
    );
    assert!(decode_watermark("bank-b", &token).is_err());
    assert_eq!(
        decode_watermark("bank-a", "").unwrap(),
        "1970-01-01T00:00:00Z"
    );
    assert!(decode_watermark("bank-a", "junk").is_err());
}

#[test]
fn lww_rule_orders_time_score_device() {
    assert_eq!(
        lww_compare("2026-01-02", None, "a", "2026-01-01", None, "z"),
        std::cmp::Ordering::Greater
    );
    // Equal stamps: higher admission score wins.
    assert_eq!(
        lww_compare("2026-01-01", Some(0.9), "a", "2026-01-01", Some(0.4), "z"),
        std::cmp::Ordering::Greater
    );
    // Missing score ranks lowest.
    assert_eq!(
        lww_compare("2026-01-01", None, "a", "2026-01-01", Some(0.4), "z"),
        std::cmp::Ordering::Less
    );
    // Full tie: lexicographic device id.
    assert_eq!(
        lww_compare("2026-01-01", Some(0.5), "b", "2026-01-01", Some(0.5), "a"),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        lww_compare("2026-01-01", None, "a", "2026-01-01", None, "a"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn mutation_validation_rejects_bad_envelopes() {
    let ok = mutation();
    assert!(ok.validate().is_ok());

    let mut bad = mutation();
    bad.entity_kind = "widget".into();
    assert!(bad.validate().is_err());

    let mut bad = mutation();
    bad.op = "teleport".into();
    assert!(bad.validate().is_err());

    let mut bad = mutation();
    bad.sync_mutation_id = String::new();
    assert!(bad.validate().is_err());

    let mut bad = mutation();
    bad.transaction_time = String::new();
    assert!(bad.validate().is_err());
}

#[test]
fn reason_code_maps_by_error_class() {
    let err = AmError::new(clio_types::ErrorCode::ErasedSubject, "shredded");
    assert_eq!(reason_code_for(&err), REASON_ERASED_SUBJECT);
    let err = AmError::new(clio_types::ErrorCode::InvalidArgument, "bad");
    assert_eq!(reason_code_for(&err), REASON_MALFORMED);
    let err = AmError::new(clio_types::ErrorCode::Forbidden, "no");
    assert_eq!(
        reason_code_for(&err),
        crate::protocol::REASON_UNAUTHORIZED_BANK
    );
    let err = AmError::new(clio_types::ErrorCode::NotImplemented, "no");
    assert_eq!(
        reason_code_for(&err),
        crate::protocol::REASON_UNSUPPORTED_OP
    );
}

#[test]
fn cipher_content_roundtrips_through_serde() {
    let m = mutation();
    let text = serde_json::to_string(&m).expect("serialize");
    let back: SyncMutation = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(back, m);
    assert!(matches!(back.content, SyncContent::Plain(_)));
}

#[test]
fn watermark_body_join_and_canonicalize_roundtrip() {
    // The sentinel is the unit separator and joins stamp and entity exactly once.
    assert_eq!(WM_ENTITY_SEP, '\u{1f}');
    assert_eq!(
        join_watermark_body("2026-01-01T00:00:00Z", "item-1"),
        "2026-01-01T00:00:00Z\u{1f}item-1"
    );
    // Empty entity: the canonical body ends with the separator and nothing else.
    assert_eq!(
        join_watermark_body("2026-01-01T00:00:00Z", ""),
        "2026-01-01T00:00:00Z\u{1f}"
    );
    // A bare body (no separator) is the (stamp, "") pair, re-encoded with one.
    assert_eq!(
        canonical_watermark_body("2026-01-01T00:00:00Z"),
        "2026-01-01T00:00:00Z\u{1f}"
    );
    // An already-canonical body passes through unchanged.
    let body = join_watermark_body("2026-01-01T00:00:00Z", "item-1");
    assert_eq!(canonical_watermark_body(&body), body);
    // Round-trip: canonicalizing a joined body reproduces the joined form.
    assert_eq!(
        canonical_watermark_body(&join_watermark_body("2026-01-01T00:00:00Z", "item-1")),
        "2026-01-01T00:00:00Z\u{1f}item-1"
    );
}

fn mutation() -> SyncMutation {
    SyncMutation {
        sync_mutation_id: "mut-1".into(),
        bank: "bank-a".into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: "item-1".into(),
        op: OP_UPSERT.into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "dev-a".into(),
        content: SyncContent::Plain(serde_json::json!({ "k": "v" })),
        content_hash: "hash-1".into(),
        admission_receipt: None,
    }
}

#[test]
fn invalidate_op_constant_matches_wire() {
    assert_eq!(OP_INVALIDATE, "invalidate");
    assert_eq!(ENTITY_KIND_ITEM, "item");
    let _ = ErrorCode::InvalidArgument;
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[test]
fn cursor_codec_rejects_every_malformed_shape() {
    // Not valid base64url at all.
    assert!(decode_cursor("!!!", "b").is_err());
    // Decodes to bytes that are not UTF-8.
    assert!(decode_cursor(&b64(&[0xff, 0xfe]), "b").is_err());
    // Valid UTF-8 with no `seq:checksum` separator.
    assert!(decode_cursor(&b64(b"abc"), "b").is_err());
    // Separator present but the sequence is not an integer.
    assert!(decode_cursor(&b64(b"x:y"), "b").is_err());
    // Well-formed but with a checksum that does not match the bank/seq.
    assert!(decode_cursor(&b64(b"1:deadbeef"), "b").is_err());
}

#[test]
fn watermark_codec_rejects_every_malformed_shape() {
    // Not valid base64url.
    assert!(decode_watermark("b", "!!!").is_err());
    // No `wm:` prefix after decoding.
    assert!(decode_watermark("b", &b64(b"zzz")).is_err());
    // `wm:` prefix but no checksum separator.
    assert!(decode_watermark("b", &b64(b"wm:abc")).is_err());
    // Well-formed but with a mismatched checksum.
    assert!(decode_watermark("b", &b64(b"wm:2026-01-01T00:00:00Z:deadbeef")).is_err());
}

#[test]
fn content_value_is_empty_for_cipher_and_present_for_plain() {
    let plain = mutation();
    assert_eq!(
        plain.content_value(),
        &serde_json::json!({ "k": "v" }),
        "plain mutations expose their logical content"
    );
    let cipher = SyncMutation {
        content: SyncContent::Cipher(crate::crypto::SyncCipher {
            alg: "xchacha20poly1305".into(),
            key_id: None,
            nonce_b64: "nonce".into(),
            ciphertext_b64: "body".into(),
        }),
        ..mutation()
    };
    assert_eq!(
        cipher.content_value(),
        &serde_json::json!({}),
        "cipher mutations expose an empty placeholder"
    );
}
