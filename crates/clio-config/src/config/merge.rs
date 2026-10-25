// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for JSON overlay flatten/merge helpers for effective config.
//!
//! ## Owns
//! - Dot-path overlay maps and `set_path` / flatten / diff helpers
//! - System default JSON document for the merge base
//!
//! ## Does not own
//! - Which overlays `Runtime` applies or in what order
//! - Config tool request/response types
//!
//! ## Boundary
//!
//! Changes to overlay merge primitives should normally require changes here.
//! Changes to tool handlers or validation policy should not require changes here.
//!
//! Keep this module focused on JSON overlay flatten and merge helpers.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// In-memory overlay map (dot-path → JSON value).
pub(crate) type Overlay = BTreeMap<String, Value>;

#[must_use]
pub(crate) fn system_defaults<F>(get: &F) -> Value
where
    F: Fn(&str) -> Option<String>,
{
    let resolved = crate::db_path::resolve_database(get, None, None);
    let sqlite_path = crate::db_path::default_sqlite_path(get)
        .to_string_lossy()
        .into_owned();
    json!({
        "backend": resolved.backend,
        "store": {
            "database_url": resolved.url,
            "sqlite_path": sqlite_path
        },
        "extract": {
            "provider": "template",
            "url": "",
            "model": "numind/NuExtract-1.5-tiny",
            "temperature": 0.0
        },
        "embed": {
            "url": "",
            "model": "BAAI/bge-small-en-v1.5",
            "dims": 384,
            "provider": "",
            "switch_space": false
        },
        "rerank": {
            "url": "",
            "model": "",
            "provider": "",
            "max_batch": 32
        },
        "log": { "level": "info" },
        "injection": { "budget_tokens": 2048 },
        "history": {
            "lesson_max_tokens": clio_types::DEFAULT_LESSON_MAX_TOKENS,
            "similarity_threshold": clio_types::DEFAULT_TASK_SIMILARITY_THRESHOLD,
            "default_limit": clio_types::DEFAULT_HISTORY_LIMIT,
            "max_limit": clio_types::MAX_HISTORY_LIMIT
        },
        "credentials": {},
        "sync": { "omit": true },
        "mcp": {
            "revision": "2025-11-25",
            "transports": ["stdio", "streamable_http"]
        },
        "fr10_satisfied": false,
        "schema_version": 1,
        "gates": {
            "admission_enabled": true,
            "category_enabled": true
        }
    })
}

pub(crate) fn flatten_object(prefix: &str, map: &Map<String, Value>, out: &mut Overlay) {
    for (k, v) in map {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Object(child) => flatten_object(&path, child, out),
            other => {
                out.insert(path, other.clone());
            }
        }
    }
}

pub(crate) fn apply_overlay(root: &mut Value, overlay: &Overlay) {
    for (path, value) in overlay {
        set_path(root, path, value.clone());
    }
}

pub(crate) fn set_path(root: &mut Value, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    let mut cur = root;
    for (i, part) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            ensure_object(cur);
            cur.as_object_mut()
                .expect("object after ensure")
                .insert((*part).to_owned(), value);
            return;
        }
        ensure_object(cur);
        let obj = cur.as_object_mut().expect("object after ensure");
        if !obj.contains_key(*part) {
            obj.insert((*part).to_owned(), Value::Object(Map::new()));
        }
        cur = obj.get_mut(*part).expect("just inserted or present");
    }
}

fn ensure_object(value: &mut Value) {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
}

#[must_use]
pub(crate) fn path_to_pointer(path: &str) -> String {
    let mut p = String::new();
    for part in path.split('.').filter(|s| !s.is_empty()) {
        p.push('/');
        p.push_str(&part.replace('~', "~0").replace('/', "~1"));
    }
    p
}

#[must_use]
pub(crate) fn value_as_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[must_use]
pub(crate) fn json_diff(before: &Value, after: &Value) -> Value {
    let mut changed = Map::new();
    let mut before_flat = Overlay::new();
    let mut after_flat = Overlay::new();
    if let Value::Object(m) = before {
        flatten_object("", m, &mut before_flat);
    }
    if let Value::Object(m) = after {
        flatten_object("", m, &mut after_flat);
    }
    let mut keys = BTreeSet::new();
    for k in before_flat.keys().chain(after_flat.keys()) {
        keys.insert(k.as_str());
    }
    for k in &keys {
        let b = before_flat.get(*k);
        let a = after_flat.get(*k);
        if b != a {
            changed.insert((*k).to_owned(), json!({ "before": b, "after": a }));
        }
    }
    Value::Object(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_path_empty_is_noop() {
        let mut root = json!({"a": 1});
        set_path(&mut root, "", json!(2));
        set_path(&mut root, "...", json!(2));
        assert_eq!(root, json!({"a": 1}));
    }

    #[test]
    fn set_path_replaces_scalar_parent() {
        let mut root = json!({"a": 1});
        set_path(&mut root, "a.b", json!(true));
        assert_eq!(root["a"]["b"], json!(true));
    }

    #[test]
    fn path_to_pointer_escapes() {
        assert_eq!(path_to_pointer("a~b/c"), "/a~0b~1c");
    }

    #[test]
    fn value_as_string_non_string() {
        assert_eq!(value_as_string(&json!(42)), "42");
    }

    #[test]
    fn json_diff_ignores_non_objects() {
        assert_eq!(json_diff(&json!(1), &json!(2)), json!({}));
    }

    #[test]
    fn system_defaults_follow_the_resolver() {
        let get = |key: &str| match key {
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        let defaults = system_defaults(&get);
        assert_eq!(defaults["backend"], json!("sqlite"));
        assert_eq!(
            defaults["store"]["sqlite_path"],
            json!("/home/u/.local/share/clio/clio.db")
        );
        assert_eq!(
            defaults["store"]["database_url"],
            json!("/home/u/.local/share/clio/clio.db")
        );

        let with_url = |key: &str| match key {
            "CLIO_DATABASE_URL" => Some("postgres://h/db".to_owned()),
            "HOME" => Some("/home/u".to_owned()),
            _ => None,
        };
        let defaults = system_defaults(&with_url);
        assert_eq!(defaults["backend"], json!("postgres"));
        assert_eq!(defaults["store"]["database_url"], json!("postgres://h/db"));
        assert_eq!(
            defaults["store"]["sqlite_path"],
            json!("/home/u/.local/share/clio/clio.db")
        );
    }
}
