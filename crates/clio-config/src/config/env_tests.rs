// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the process-env overlay on
//! `Runtime`: which variables reach which config paths, and how a bearer
//! credential is picked.
//!
//! ## Owns
//! - Every variable-to-config-path mapping the overlay applies
//! - The overlay clear, the unparseable-number branch, and the repo-root field
//! - Bearer credential precedence through both entry points
//!
//! ## Does not own
//! - The overlay implementation (see `config::env`)
//! - Which variable names a database (see `db_url_env`)
//! - Config defaults, validation, and the config-file merge (see `config`)
//!
//! ## Boundary
//!
//! Tests here break when the overlay stops mapping a variable to the path it
//! used to; they never change the implementation.
//!
//! Keep this module focused on process-env overlay behavior.

use super::*;
use crate::db_url_env::DATABASE_URL_ENV;
use serde_json::json;

/// Build an injected environment lookup from key/value pairs.
fn env(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + 'static {
    let pairs: Vec<(&'static str, &'static str)> = pairs.to_vec();
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

/// One dotted config path, read as text, out of the overlay map.
///
/// The map is read rather than the merged document: a merge would fold in
/// whatever config file the host happens to have, which is not what these tests
/// are about.
fn text(rt: &Runtime, path: &str) -> Option<String> {
    let entry = rt.env_overlay.get(path)?;
    entry.as_str().map(str::to_owned)
}

/// Every variable the overlay reads, with a value each mapping can be checked
/// against. One table rather than twenty literals, so a variable added to the
/// overlay cannot quietly escape these tests.
fn all_pairs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("CLIO_LOG_LEVEL", "debug"),
        ("CLIO_BACKEND", "postgres"),
        (DATABASE_URL_ENV, "postgres://env/db"),
        ("CLIO_EMBED_URL", "http://127.0.0.1:1/embed"),
        ("CLIO_EMBED_MODEL", "some/embed-model"),
        ("CLIO_EMBED_PROVIDER", "openai"),
        ("CLIO_EMBED_DIMS", "512"),
        ("CLIO_EMBED_API_KEY", "sk-embed-token-0123456789"),
        ("CLIO_RERANK_URL", "http://127.0.0.1:1/rerank"),
        ("CLIO_RERANK_MODEL", "some/rerank-model"),
        ("CLIO_RERANK_PROVIDER", "cohere"),
        ("CLIO_RERANK_API_KEY", "rk-rerank-token-0123456789"),
        ("CLIO_EXTRACT_PROVIDER", "template"),
        ("CLIO_EXTRACT_URL", "http://127.0.0.1:1/extract"),
        ("CLIO_EXTRACT_MODEL", "some/extract-model"),
        ("CLIO_EXTRACT_API_KEY", "ek-extract-token-0123456789"),
        ("CLIO_BANK", "env-bank"),
        ("CLIO_REPO_ROOT", "/tmp/from-env"),
    ]
}

#[test]
fn every_variable_reaches_the_path_it_owns() {
    let pairs = all_pairs();
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&pairs));

    let expected = [
        ("log.level", "debug"),
        ("backend", "postgres"),
        ("store.database_url", "postgres://env/db"),
        ("embed.url", "http://127.0.0.1:1/embed"),
        ("embed.model", "some/embed-model"),
        ("embed.provider", "openai"),
        ("rerank.url", "http://127.0.0.1:1/rerank"),
        ("rerank.model", "some/rerank-model"),
        ("rerank.provider", "cohere"),
        ("extract.provider", "template"),
        ("extract.url", "http://127.0.0.1:1/extract"),
        ("extract.model", "some/extract-model"),
        ("bank.fixed", "env-bank"),
    ];
    for (path, value) in expected {
        let found = text(&rt, path);
        assert_eq!(found.as_deref(), Some(value), "got the wrong value");
    }

    let secrets = [
        ("credentials.embed_api_key", "sk-embed-token-0123456789"),
        ("credentials.rerank_api_key", "rk-rerank-token-0123456789"),
        ("credentials.extract_api_key", "ek-extract-token-0123456789"),
    ];
    for (path, value) in secrets {
        let found = text(&rt, path);
        assert_eq!(found.as_deref(), Some(value), "got the wrong value");
    }

    assert_eq!(rt.repo_root.as_deref(), Some("/tmp/from-env"));
}

/// A dimension cannot stay text in a config document, so it is folded in as a
/// number rather than as the characters that were read.
#[test]
fn a_dimension_is_folded_in_as_a_number() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&[("CLIO_EMBED_DIMS", "512")]));
    assert_eq!(rt.env_overlay.get("embed.dims"), Some(&json!(512_u64)));
}

/// A dimension that is not a number cannot become one, so the overlay leaves the
/// path alone and the config default stands.
#[test]
fn an_unparseable_dimension_leaves_the_path_alone() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&[("CLIO_EMBED_DIMS", "not-a-number")]));
    assert!(!rt.env_overlay.contains_key("embed.dims"));
}

/// The database URL is the one variable every code path has to agree on, so the
/// overlay reads it through its owner instead of naming it here.
#[test]
fn the_database_url_reaches_the_store_path_through_its_owner() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&[(DATABASE_URL_ENV, "postgres://env/db")]));
    let found = text(&rt, "store.database_url");
    assert_eq!(found.as_deref(), Some("postgres://env/db"));
}

/// The bare name is not a second spelling any more, so an operator who exports
/// only it gets no URL from the overlay.
#[test]
fn the_bare_database_url_name_is_not_read() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&[("DATABASE_URL", "postgres://bare/db")]));
    assert!(!rt.env_overlay.contains_key("store.database_url"));
}

/// The bare names are not second spellings for the three services either, so an
/// operator who exports only those gets an empty overlay and every knob keeps
/// its config default. All thirteen go in together, because a single name would
/// only prove that one name is unread.
#[test]
fn the_bare_provider_names_are_not_read() {
    let bare = [
        ("EMBED_URL", "http://127.0.0.1:1/embed"),
        ("EMBED_MODEL", "some/embed-model"),
        ("EMBED_PROVIDER", "openai"),
        ("EMBED_DIMS", "512"),
        ("EMBED_API_KEY", "sk-embed-token-0123456789"),
        ("RERANK_URL", "http://127.0.0.1:1/rerank"),
        ("RERANK_MODEL", "some/rerank-model"),
        ("RERANK_PROVIDER", "cohere"),
        ("RERANK_API_KEY", "rk-rerank-token-0123456789"),
        ("EXTRACT_PROVIDER", "template"),
        ("EXTRACT_URL", "http://127.0.0.1:1/extract"),
        ("EXTRACT_MODEL", "some/extract-model"),
        ("EXTRACT_API_KEY", "ek-extract-token-0123456789"),
    ];
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&bare));
    assert!(
        rt.env_overlay.is_empty(),
        "a bare variable name reached the overlay: {:?}",
        rt.env_overlay
    );
}

/// A second apply replaces the overlay rather than adding to it, so a variable
/// that leaves the environment leaves nothing behind.
#[test]
fn a_second_apply_replaces_the_overlay() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&[("CLIO_EMBED_URL", "http://127.0.0.1:1/first")]));
    assert!(rt.env_overlay.contains_key("embed.url"));
    rt.apply_process_env(&mut env(&[("CLIO_EMBED_MODEL", "some/model")]));
    assert!(!rt.env_overlay.contains_key("embed.url"));
    assert!(rt.env_overlay.contains_key("embed.model"));
}

#[test]
fn an_empty_environment_leaves_the_overlay_empty() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut |_key: &str| None);
    let size = rt.env_overlay.len();
    assert_eq!(
        size, 0,
        "no variable, no overlay entry: {:?}",
        rt.env_overlay
    );
}

/// A per-service key outranks the generic one.
#[test]
fn a_service_key_outranks_the_generic_key() {
    let doc = json!({
        "credentials": {
            "api_key": "generic-token-0123456789",
            "embed_api_key": "embed-token-0123456789"
        }
    });
    let found = bearer_for_effective(&doc, "embed");
    assert_eq!(found.as_deref(), Some("embed-token-0123456789"));
    let fallback = bearer_for_effective(&doc, "rerank");
    assert_eq!(fallback.as_deref(), Some("generic-token-0123456789"));
}

/// An empty string is not a credential, so the next tier applies rather than the
/// open handing an empty key to a service.
#[test]
fn an_empty_service_key_falls_through_to_the_generic_key() {
    let doc = json!({
        "credentials": {"api_key": "generic-token-0123456789", "rerank_api_key": ""}
    });
    let found = bearer_for_effective(&doc, "rerank");
    assert_eq!(found.as_deref(), Some("generic-token-0123456789"));
}

#[test]
fn a_document_with_no_credentials_yields_no_bearer() {
    assert_eq!(bearer_for_effective(&json!({}), "embed"), None);
}

/// The service key the overlay reads is the one `bearer_for` hands back, which
/// is what makes an env-supplied sidecar credential usable with no config file.
/// Only the service-specific path is asserted: the generic key is a lower tier a
/// host config file may also set.
#[test]
fn a_service_key_from_the_environment_is_the_bearer() {
    let mut rt = Runtime::with_env(&|_| None);
    rt.apply_process_env(&mut env(&[(
        "CLIO_EMBED_API_KEY",
        "sk-embed-token-0123456789",
    )]));
    let found = rt.bearer_for("embed");
    assert_eq!(found.as_deref(), Some("sk-embed-token-0123456789"));
}
