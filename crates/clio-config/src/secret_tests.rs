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
fn path_is_secret_matches_whole_name_segments() {
    // Whole-token matching: a marker must appear as a contiguous token run.
    for name in [
        "auth",
        "user_auth",
        "auth_token",
        "authToken",
        "api_key",
        "user_api_key",
        "apikey",
        "apiKey",
        "private_key",
        "database_url",
        "password",
        "token",
        "credential",
        "secret",
        "dek",
        "dek_ref",
        "credentials.api_key",
        "store.database_url",
        "items[0].password",
    ] {
        assert!(path_is_secret(name), "{name} must be secret");
    }
    for name in [
        "author",
        "authored_at",
        "credentials",
        "tokens",
        "lesson_max_tokens",
        "budget_tokens",
        "dekstop",
        "history.lesson_max_tokens",
        "injection.budget_tokens",
    ] {
        assert!(!path_is_secret(name), "{name} must not be secret");
    }
}

#[test]
fn mask_value_redacts_nested_api_key() {
    let mut v = json!({
        "backend": "sqlite",
        "credentials": { "api_key": "sk-SECRETKEY99" }
    });
    mask_value("", &mut v);
    // The field name is kept so a path read still finds it.
    assert_eq!(v["credentials"]["api_key"], json!("****EY99"));
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
    // A number under a strong secret name is masked as a whole literal.
    assert_eq!(v["items"][0]["password"], json!("[REDACTED]"));
    assert_eq!(v["items"][1]["ok"], json!(true));
    // `token` alone also names counts, so a null stays null.
    assert_eq!(v["token"], json!(null));
    let _ = masked_clone(&json!({"secret": "ab"}));
}

#[test]
fn non_string_leaves_are_masked_under_strong_secret_names() {
    let mut v = json!({
        "password": 123_456,
        "api_key": 99_887_766,
        "auth_pin": 4821,
        "password_set": true,
        "lesson_max_tokens": 32000,
        "budget_tokens": 2048,
        "secret": 0.25
    });
    mask_value("", &mut v);
    for name in ["password", "api_key", "auth_pin", "password_set"] {
        assert_eq!(v[name], json!("[REDACTED]"), "{name} leaked: {v}");
    }
    // Count- and weight-shaped names keep their number.
    assert_eq!(v["lesson_max_tokens"], json!(32000));
    assert_eq!(v["budget_tokens"], json!(2048));
    assert_eq!(v["secret"], json!(0.25));
}

#[test]
fn masked_clone_masks_a_secret_shaped_key() {
    // A secret stored in the key name must not survive the clone, while an
    // ordinary sibling entry is untouched.
    let rendered = masked_clone(&json!({"entity": "Ada", "sk-live-SECRETKEY99": "x"})).to_string();
    assert!(!rendered.contains("sk-live-SECRETKEY99"), "{rendered}");
    assert!(rendered.contains("\"entity\":\"Ada\""), "{rendered}");
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
}

#[test]
fn masked_clone_keeps_every_entry_when_masked_keys_collide() {
    // Two secret-shaped keys mask to the same name; neither entry is dropped.
    let masked = masked_clone(&json!({
        "sk-live-SECRETAAAA1": "a",
        "sk-live-SECRETBBBB2": "b",
        "entity": "Ada"
    }));
    let object = masked.as_object().expect("masked document");
    assert_eq!(object.len(), 3, "{object:?}");
    assert_eq!(object["[REDACTED]"], json!("[REDACTED]"));
    assert_eq!(object["[REDACTED]#2"], json!("[REDACTED]"));
    assert_eq!(object["entity"], json!("Ada"));
}

#[test]
fn masked_clone_keeps_config_field_names() {
    let document = json!({
        "credentials": {"api_key": "sk-live-abcdef0123456789"},
        "history": {"lesson_max_tokens": 32000, "default_limit": 20},
        "injection": {"budget_tokens": 2048},
        "ranking": {"hygiene": {"secret": 0.25}},
        "store": {"database_url": "postgres://user:pw@127.0.0.1/db"}
    });
    let masked = masked_clone(&document);
    let keys: Vec<&String> = masked
        .as_object()
        .expect("masked document")
        .keys()
        .collect();
    assert_eq!(
        keys,
        ["credentials", "history", "injection", "ranking", "store"],
        "{masked}"
    );
    let object = masked.as_object().expect("masked document");
    for section in ["credentials", "history", "injection", "ranking", "store"] {
        assert!(object.contains_key(section), "{section} vanished");
    }
    assert!(
        object["history"].get("lesson_max_tokens").is_some(),
        "{masked}"
    );
    assert!(
        object["injection"].get("budget_tokens").is_some(),
        "{masked}"
    );
    assert!(
        object["ranking"]["hygiene"].get("secret").is_some(),
        "{masked}"
    );
    assert!(
        object["store"].get("database_url").is_some(),
        "database_url was renamed: {masked}"
    );

    // Non-string leaves keep their exact value, even under a marker name.
    assert_eq!(masked["history"]["lesson_max_tokens"], json!(32000));
    assert_eq!(masked["history"]["default_limit"], json!(20));
    assert_eq!(masked["injection"]["budget_tokens"], json!(2048));
    assert_eq!(masked["ranking"]["hygiene"]["secret"], json!(0.25));

    // String secrets are masked in place, under their own names.
    let api_key = masked["credentials"]["api_key"]
        .as_str()
        .expect("masked api_key");
    assert_ne!(api_key, "sk-live-abcdef0123456789");
    assert_eq!(api_key, "****6789");
    let database_url = masked["store"]["database_url"]
        .as_str()
        .expect("masked database_url");
    assert_ne!(database_url, "postgres://user:pw@127.0.0.1/db");
    assert_eq!(database_url, "****1/db");
}

#[test]
fn masked_at_path_masks_strings_and_passes_scalars_through() {
    assert_eq!(
        masked_at_path("history.lesson_max_tokens", &json!(32000)),
        json!(32000)
    );
    assert_eq!(
        masked_at_path("injection.budget_tokens", &json!(2048)),
        json!(2048)
    );
    assert_eq!(
        masked_at_path("ranking.hygiene.secret", &json!(0.25)),
        json!(0.25)
    );
    let masked = masked_at_path("credentials.api_key", &json!("sk-live-abcdef0123456789"));
    assert_eq!(masked, json!("****6789"));
    let nested = masked_at_path(
        "credentials",
        &json!({"api_key": "sk-live-abcdef0123456789", "region": "eu"}),
    );
    assert_eq!(nested["api_key"], json!("****6789"));
    assert_eq!(nested["region"], json!("eu"));
}

#[test]
fn masked_at_path_masks_a_url_that_carries_credentials() {
    // A URL is not secret by name; only the value that carries `user:pw@` is.
    let raw = "https://user:pw@host/v1/embeddings";
    assert!(!path_is_secret("embed.url"));
    let masked = masked_at_path("embed.url", &json!(raw));
    assert_eq!(masked, json!("****ings"), "{masked}");
    let tree = masked_clone(&json!({"embed": {"url": raw, "model": "m"}}));
    assert_eq!(tree["embed"]["url"], json!("****ings"), "{tree}");
    assert_eq!(tree["embed"]["model"], json!("m"));
}

#[test]
fn masked_at_path_leaves_a_url_without_credentials_unchanged() {
    let raw = "https://host/v1/embeddings";
    assert_eq!(masked_at_path("embed.url", &json!(raw)), json!(raw));
    let tree = masked_clone(&json!({"embed": {"url": raw}}));
    assert_eq!(tree["embed"]["url"], json!(raw), "{tree}");
}

#[test]
fn value_is_secret_reads_userinfo_credentials_from_the_value() {
    assert!(value_is_secret(
        "embed.url",
        "postgres://user:pw@host:5432/db"
    ));
    assert!(value_is_secret("embed.url", "https://:pw@host/db"));
    assert!(!value_is_secret("embed.url", "https://user@host/db"));
    assert!(!value_is_secret("embed.url", "https://host/a@b"));
    assert!(!value_is_secret("embed.url", "not a url"));
    assert!(value_is_secret("credentials.api_key", "sk-abcdefgh"));
}

#[test]
fn mask_string_secrets_masks_strings_and_keeps_non_strings() {
    // `mask_string_secrets` is the view-safe walk: it masks secret-keyed string
    // leaves but must not rewrite non-string leaves or rename keys, so a numeric
    // counter such as `lesson_tokens` keeps its type and its name.
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
    let once = masked_clone(&json!({
        "my_auth": "ab",
        "credentials": {"api_key": "xy", "database_url": "postgres://u:p@h/db"},
        "items": [{"secret": "abcd"}],
        "hygiene": {"secret": 0.25}
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
