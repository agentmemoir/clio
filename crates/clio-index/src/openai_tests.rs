// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the OpenAI-compatible embeddings
//! adapter: request/response shape, index-ordered reconstruction, and the
//! fail-closed count/width gates.
//!
//! ## Owns
//! - OpenAI adapter unit and mock-server assertions
//!
//! ## Does not own
//! - Production adapter code (`openai`) or the shared fake server (`embed_test_support`)
//!
//! ## Boundary
//!
//! Changes to the OpenAI dialect should normally update the suite here.
//! Changes to the TEI dialect should not require changes here.

use super::*;
use crate::embed_test_support::{FakeEmbedServer, fake_vector};
use serde_json::json;

fn raw_response(body: &Value) -> Vec<u8> {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn embedding(values: &[f32]) -> Value {
    serde_json::to_value(values).expect("vector serializes")
}

#[test]
fn batch_is_reconstructed_in_input_order() {
    let first = fake_vector("first", 8);
    let second = fake_vector("second", 8);
    // The provider returns the entries in reverse index order.
    let body = json!({
        "object": "list",
        "data": [
            {"object": "embedding", "index": 1, "embedding": embedding(&second)},
            {"object": "embedding", "index": 0, "embedding": embedding(&first)}
        ]
    });
    let server = FakeEmbedServer::spawn_raw(raw_response(&body));
    let mut embedder =
        OpenAiEmbedder::new(server.url.clone(), "text-embedding-3-small", 8, None).unwrap();
    embedder.set_timeout(Duration::from_secs(5));
    let out = embedder
        .embed_batch(&["first".to_owned(), "second".to_owned()])
        .unwrap();
    assert_eq!(out[0], first);
    assert_eq!(out[1], second);
    assert_eq!(server.served(), 1);
    assert_eq!(embedder.model_id(), "text-embedding-3-small");
    assert_eq!(embedder.dims(), 8);
}

#[test]
fn empty_batch_skips_the_endpoint() {
    let server = FakeEmbedServer::spawn();
    let embedder = OpenAiEmbedder::new(server.url.clone(), "m", 8, None).unwrap();
    let vectors = embedder.embed_batch(&[]).unwrap();
    assert!(vectors.is_empty(), "{vectors:?}");
    assert_eq!(server.served(), 0);
}

#[test]
fn wrong_width_and_count_fail_closed() {
    let body = json!({"data": [{"index": 0, "embedding": embedding(&[0.1_f32; 7])}]});
    let server = FakeEmbedServer::spawn_raw(raw_response(&body));
    let embedder = OpenAiEmbedder::new(server.url.clone(), "m", 8, None).unwrap();
    let err = embedder.embed_batch(&["x".to_owned()]).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(err.message.contains("dim mismatch fails closed"), "{err}");

    let body = json!({
        "data": [
            {"index": 0, "embedding": embedding(&[0.1_f32; 8])},
            {"index": 1, "embedding": embedding(&[0.2_f32; 8])}
        ]
    });
    let server = FakeEmbedServer::spawn_raw(raw_response(&body));
    let embedder = OpenAiEmbedder::new(server.url.clone(), "m", 8, None).unwrap();
    let err = embedder.embed_batch(&["x".to_owned()]).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(err.message.contains("does not match batch size"), "{err}");
}

#[test]
fn health_probe_uses_the_models_endpoint() {
    let server = FakeEmbedServer::spawn();
    let embedder = OpenAiEmbedder::new(server.url.clone(), "m", 8, None).unwrap();
    assert!(embedder.health().is_ok());
    assert_eq!(server.served(), 1);

    let dead = OpenAiEmbedder::new("http://127.0.0.1:1", "m", 8, None).unwrap();
    assert!(dead.health().is_err());
}

#[test]
fn construction_fails_closed_on_bad_config() {
    assert_eq!(
        OpenAiEmbedder::new("", "m", 8, None).unwrap_err().code,
        ErrorCode::ConfigCorrupt
    );
    assert_eq!(
        OpenAiEmbedder::new("http://127.0.0.1:1", "", 8, None)
            .unwrap_err()
            .code,
        ErrorCode::ConfigCorrupt
    );
    assert_eq!(
        OpenAiEmbedder::new("http://127.0.0.1:1", "m", 0, None)
            .unwrap_err()
            .code,
        ErrorCode::ConfigCorrupt
    );
    assert_eq!(
        OpenAiEmbedder::new("ftp://nope", "m", 8, None)
            .unwrap_err()
            .code,
        ErrorCode::ConfigCorrupt
    );
}

#[test]
fn response_shape_failures_are_rejected() {
    let v = embedding(&[0.1_f32; 4]);
    let cases = [
        (json!({"nope": []}), ErrorCode::Internal),
        (json!({"data": []}), ErrorCode::OutOfRange),
        (json!({"data": [{"embedding": v}]}), ErrorCode::Internal),
        (
            json!({"data": [{"index": 5, "embedding": v}]}),
            ErrorCode::OutOfRange,
        ),
        (
            json!({"data": [{"index": 0, "embedding": v}, {"index": 0, "embedding": v}]}),
            ErrorCode::OutOfRange,
        ),
        (
            json!({"data": [{"index": 0, "embedding": "nope"}]}),
            ErrorCode::Internal,
        ),
    ];
    for (value, expected) in cases {
        let err = parse_openai_embeddings(&value, 1, 4, "m").unwrap_err();
        assert_eq!(err.code, expected, "{err}");
    }
    // A missing slot (index 1 only) is rejected rather than silently padded.
    let err = parse_openai_embeddings(
        &json!({"data": [{"index": 1, "embedding": embedding(&[0.1_f32; 4])}]}),
        2,
        4,
        "m",
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
}
