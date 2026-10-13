// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing embed text assembly, redaction,
//! dimension gating, and the TEI client against the fake sidecar.
//!
//! ## Owns
//! - Embed-text selection, secret scrubbing, and dim-mismatch failure cases
//! - TEI client request/response shapes and bearer configuration
//!
//! ## Does not own
//! - Raw HTTP transport edge cases (see `http_tests`)
//! - Worker/rebuild policy (see `worker_tests`/`rebuild_tests`)
//!
//! ## Boundary
//!
//! Changes to embed text or client behavior should normally update tests here.
//! Changes to index scheduling should not require changes here.
//!
//! Keep this module focused on embed pipeline behavior.

use crate::embed::{Embedder, TeiEmbedder, embed_text, lexical_text, scrub_embed_text};
use crate::embed_test_support::{FakeEmbedServer, fake_vector};
use clio_types::{EpistemicKind, ItemKind};
use serde_json::json;
use std::time::Duration;

fn item(gist: Option<&str>, snapshot: Option<serde_json::Value>) -> clio_types::MemoryItem {
    clio_types::MemoryItem {
        id: "itm-1".into(),
        bank_id: "b".into(),
        subject_id: "s".into(),
        kind: ItemKind::Semantic,
        category: None,
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot,
        gist: gist.map(str::to_owned),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

// The descriptive source `context` is retrieval metadata, never a dense or
// lexical signal (deterministic default policy).
#[test]
fn source_context_is_excluded_from_dense_and_lexical_text() {
    let mut it = item(
        Some("release notes"),
        Some(json!({"entity": "release", "amount": 7})),
    );
    it.context = Some("classified rollout room".to_owned());
    let dense = embed_text(&it).expect("dense text");
    assert!(!dense.contains("classified rollout room"));
    let lexical = lexical_text(&it).expect("lexical text");
    assert!(!lexical.contains("classified rollout room"));
}

#[test]
fn embed_text_prefers_gist_then_snapshot() {
    let both = item(Some("prose gist"), Some(json!({"code": "ECONNRESET"})));
    assert_eq!(embed_text(&both).unwrap(), "prose gist");
    let snapshot_only = item(None, Some(json!({"error_code": "ECONNRESET", "n": 3})));
    assert_eq!(
        embed_text(&snapshot_only).unwrap(),
        r#"{"error_code":"ECONNRESET","n":3}"#
    );
    let empty = item(Some("   "), None);
    assert!(embed_text(&empty).is_none());
    let none = item(None, None);
    assert!(embed_text(&none).is_none());
}

#[test]
fn lexical_text_includes_identifier_tokens() {
    let both = item(
        Some("deploy gist"),
        Some(json!({"error_code": "ECONNRESET"})),
    );
    let text = lexical_text(&both).unwrap();
    assert!(text.contains("deploy gist"));
    assert!(
        text.contains("ECONNRESET"),
        "snapshot tokens feed exact match"
    );
    let empty = item(Some("  "), None);
    assert!(lexical_text(&empty).is_none());
}

#[test]
fn scrub_redacts_marker_adjacent_secrets() {
    let text = "config api_key=hunter2 in the deploy note";
    let scrubbed = scrub_embed_text(text);
    assert!(
        scrubbed.contains("api_key=****"),
        "marker-adjacent value must be masked"
    );
    assert_eq!(scrub_embed_text("nothing here"), "nothing here");
}

#[test]
fn tei_client_config_fails_closed() {
    assert!(TeiEmbedder::new("", "m", 384, None).is_err());
    assert!(TeiEmbedder::new("http://127.0.0.1:34311", "", 384, None).is_err());
    assert!(TeiEmbedder::new("http://127.0.0.1:34311", "m", 0, None).is_err());
    let err = TeiEmbedder::new("ftp://remote.example.com", "m", 384, None).unwrap_err();
    assert!(
        err.to_string().contains("http://"),
        "non-http(s) endpoints fail closed at construction: {err}"
    );
}

#[test]
fn fake_embed_server_deterministic_batch_and_health() {
    let server = FakeEmbedServer::spawn();
    let mut client =
        TeiEmbedder::new(server.url.clone(), "BAAI/bge-small-en-v1.5", 384, None).unwrap();
    client.set_timeout(Duration::from_secs(10));
    let texts = vec!["deploy failed".to_string(), "deploy ok".to_string()];
    let vectors = client.embed_batch(&texts).unwrap();
    assert_eq!(vectors.len(), 2);
    assert_eq!(vectors[0], fake_vector("deploy failed", 384));
    assert_eq!(vectors[1], fake_vector("deploy ok", 384));
    assert!(server.served() >= 1);
    client.health().unwrap();
    assert_eq!(client.model_id(), "BAAI/bge-small-en-v1.5");
    assert_eq!(client.dims(), 384);
    let none = client.embed_batch(&[]).unwrap();
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn tei_client_sends_bearer_when_configured() {
    let server = FakeEmbedServer::spawn();
    let client = TeiEmbedder::new(
        server.url.clone(),
        "fake/model",
        384,
        Some("sk-test-token-9876543210".into()),
    )
    .unwrap();
    client.embed_batch(&["x".to_string()]).unwrap();
    assert!(
        server
            .last_auth()
            .expect("bearer header")
            .starts_with("Bearer sk-")
    );
    // The token never appears in any structured log; only the sidecar saw it.
}

#[test]
fn server_error_is_structured() {
    let server = FakeEmbedServer::spawn_status(500);
    let client = TeiEmbedder::new(server.url.clone(), "fake", 384, None).unwrap();
    let err = client.embed_batch(&["x".to_string()]).unwrap_err();
    assert!(err.to_string().contains("HTTP 500"), "{err}");
}

#[test]
fn long_request_body_roundtrips_through_content_length() {
    // A >16 KiB input forces the fake server through its content-length
    // continuation read (the head arrives before the full body).
    let long_text = "deploy step ".repeat(2000);
    let server = FakeEmbedServer::spawn();
    let client = TeiEmbedder::new(server.url.clone(), "fake", 384, None).unwrap();
    let vectors = client.embed_batch(&[long_text]).unwrap();
    assert_eq!(vectors.len(), 1);
}

#[test]
fn single_string_input_and_missing_inputs_shapes() {
    let server = FakeEmbedServer::spawn();
    // Scalar {"inputs": "one text"} shape → one vector back.
    let value = crate::http::post_json(
        &server.url,
        &json!({"inputs": "one text"}),
        Duration::from_secs(5),
        None,
    )
    .unwrap();
    assert_eq!(value.as_array().expect("one vector").len(), 1);
    // Missing inputs → empty vector list (the embedder layer rejects the count).
    let value =
        crate::http::post_json(&server.url, &json!({}), Duration::from_secs(5), None).unwrap();
    assert_eq!(value.as_array().expect("empty list").len(), 0);
}
