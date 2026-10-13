// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::json;

#[test]
fn mask_secret_keeps_last_four() {
    assert_eq!(mask_secret("sk-abcdefgh"), "****efgh");
    assert_eq!(mask_secret(""), "[REDACTED]");
    assert_eq!(mask_secret("ab"), "[REDACTED]");
    assert_eq!(mask_secret("1234"), "[REDACTED]");
    assert_eq!(mask_secret("12345"), "****2345");
}

#[test]
fn mask_secret_keeps_the_redacted_literal() {
    assert_eq!(mask_secret("[REDACTED]"), "[REDACTED]");
}

#[test]
fn mask_value_redacts_nested_api_key() {
    let mut v = json!({
        "backend": "sqlite",
        "credentials": { "api_key": "sk-SECRETKEY99" }
    });
    mask_value("", &mut v);
    // The secret-shaped field name is masked along with its value.
    assert_eq!(v["****ials"]["****_key"], json!("****EY99"));
    assert_eq!(v["backend"], json!("sqlite"));
}

#[test]
fn mask_arrays_and_non_string_secrets() {
    assert!(path_is_secret("auth.token"));
    let mut v = json!({
        "items": [{"password": 123}, {"ok": true}],
        "token": null
    });
    mask_value("", &mut v);
    assert_eq!(v["items"][0]["****word"], json!("[REDACTED]"));
    assert_eq!(v["items"][1]["ok"], json!(true));
    assert_eq!(v["****oken"], json!("[REDACTED]"));
    let _ = masked_clone(&json!({"secret": "ab"}));
}

#[test]
fn mask_value_keeps_every_entry_when_masked_keys_collide() {
    // `embed_api_key`, `extract_api_key`, and `rerank_api_key` all mask to the
    // same last-4 form; every entry must still be present, not overwritten.
    let mut v = json!({
        "credentials": {
            "embed_api_key": "a",
            "extract_api_key": "b",
            "rerank_api_key": "c"
        }
    });
    mask_value("", &mut v);
    let section = v["****ials"].as_object().expect("masked section");
    assert_eq!(section.len(), 3, "{section:?}");
    assert_eq!(section["****_key"], json!("[REDACTED]"));
    assert_eq!(section["****_key#2"], json!("[REDACTED]"));
    assert_eq!(section["****_key#3"], json!("[REDACTED]"));
}

#[test]
fn masked_clone_masks_a_secret_shaped_key() {
    // The value under a secret-looking key is masked today, but the key itself
    // is echoed. A secret stored in the key name must not survive the clone.
    let document = json!({"entity": "Ada", "sk-live-SECRETKEY99": "x"});
    let rendered = serde_json::to_string(&masked_clone(&document)).unwrap();
    assert!(!rendered.contains("sk-live-SECRETKEY99"), "{rendered}");
    assert!(rendered.contains("****EY99"), "{rendered}");
    assert!(rendered.contains("\"entity\":\"Ada\""), "{rendered}");
}

#[test]
fn mask_string_secrets_masks_strings_and_keeps_non_strings() {
    // `mask_string_secrets` is the view-safe variant of `mask_value`: it masks
    // secret-keyed string leaves but must not rewrite non-string leaves, so a
    // numeric counter such as `lesson_tokens` keeps its type.
    let mut v = json!({
        "api_key": "sk-SECRETKEY99",
        "lesson_tokens": 3,
        "token": null,
        "nested": {"password": "hunter2secret", "count": 7},
        "items": [{"secret": "sk-abcdefgh"}, {"ok": true}]
    });
    mask_string_secrets("", &mut v);
    assert_eq!(v["api_key"], json!("****EY99"));
    assert_eq!(v["lesson_tokens"], json!(3));
    assert_eq!(v["token"], json!(null));
    assert_eq!(v["nested"]["password"], json!("****cret"));
    assert_eq!(v["nested"]["count"], json!(7));
    assert_eq!(v["items"][0]["secret"], json!("****efgh"));
    assert_eq!(v["items"][1]["ok"], json!(true));
}

#[test]
fn masking_an_already_masked_document_changes_nothing() {
    // `my_auth` masks to `****auth`, which still looks like a secret key, so a
    // second pass reaches its value, which the first pass left as `[REDACTED]`.
    let once = masked_clone(&json!({
        "my_auth": "ab",
        "credentials": {"api_key": "xy", "database_url": "postgres://u:p@h/db"},
        "items": [{"secret": "abcd"}]
    }));
    assert_eq!(masked_clone(&once), once, "{once}");
}

#[test]
fn mask_string_secrets_is_idempotent() {
    let mut v = json!({"api_key": "ab", "nested": {"token": "x"}});
    mask_string_secrets("", &mut v);
    let once = v.clone();
    mask_string_secrets("", &mut v);
    assert_eq!(v, once, "{v}");
}

#[test]
fn scrub_masks_every_api_key_occurrence() {
    let raw = "api_key: sk-FIRSTKEY99 then api_key: sk-SECONDKEY88";
    let scrubbed = scrub_inline_secrets(raw);
    assert!(!scrubbed.contains("sk-FIRSTKEY99"));
    assert!(!scrubbed.contains("sk-SECONDKEY88"));
    assert!(scrubbed.contains("****"));
}

#[test]
fn scrub_is_idempotent_over_an_already_masked_value() {
    // An export scrubs a short value to the literal `[REDACTED]`. Feeding that
    // masked bundle back through the scrubber (the import path) must not
    // mangle it: the masker returns `[REDACTED]` unchanged.
    let once = scrub_inline_secrets("token: ab");
    assert_eq!(once, "token: [REDACTED]");
    let twice = scrub_inline_secrets(&once);
    assert_eq!(twice, once, "re-scrubbing corrupted an existing mask");
    // A longer masked value was already a fixed point; it must stay put too.
    let long = scrub_inline_secrets("token: abcdefgh");
    assert_eq!(long, "token: ****efgh");
    assert_eq!(scrub_inline_secrets(&long), long);
}

#[test]
fn scrub_still_masks_a_fresh_secret_after_an_already_masked_one() {
    // Skipping an existing mask must not abort the scan: a real secret later
    // in the same string is still masked.
    let scrubbed = scrub_inline_secrets("token: [REDACTED] and api_key: sk-LIVEKEY42");
    assert!(scrubbed.contains("token: [REDACTED]"));
    assert!(!scrubbed.contains("sk-LIVEKEY42"));
}

#[test]
fn scrub_handles_multibyte_prose() {
    let raw = "café api_key: sk-SECRETKEY99 東京";
    let scrubbed = scrub_inline_secrets(raw);
    assert!(scrubbed.contains("café"));
    assert!(scrubbed.contains("東京"));
    assert!(!scrubbed.contains("sk-SECRETKEY99"));
}
