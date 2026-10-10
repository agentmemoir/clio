// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing provider selection in `build_embedder`:
//! the tei and OpenAI-compatible dialects, and the fail-closed empty/unknown
//! provider paths.
//!
//! ## Owns
//! - Provider-dispatch assertions against the shared fake embed server
//!
//! ## Does not own
//! - The adapter dialects themselves (`embed` / `openai`)
//! - Effective-config resolution (`clio-config` / `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to provider selection should normally update the suite here.
//! Changes to an adapter's request shape should not require changes here.

use super::*;
use crate::embed_test_support::FakeEmbedServer;
use serde_json::json;

/// Provider error without requiring `Debug` on the returned trait object.
fn err_of(provider: &str, url: &str, model: &str, dims: usize) -> AmError {
    match build_embedder(provider, url, model, dims, None) {
        Ok(_) => panic!("expected a provider configuration error"),
        Err(e) => e,
    }
}

fn openai_body(dims: usize) -> Vec<u8> {
    let body = json!({
        "object": "list",
        "data": [{"object": "embedding", "index": 0, "embedding": vec![0.5_f32; dims]}]
    })
    .to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[test]
fn tei_provider_selects_the_tei_adapter() {
    let server = FakeEmbedServer::spawn();
    let dims = clio_store::EMBEDDING_DIMS;
    let embedder = build_embedder("tei", server.url.clone(), "m", dims, None).unwrap();
    let out = embedder.embed_batch(&["x".to_owned()]).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].len(), dims);
    assert_eq!(embedder.model_id(), "m");
    assert!(server.served() >= 1);
}

#[test]
fn empty_provider_fails_closed_instead_of_selecting_a_default() {
    let err = err_of("", "http://127.0.0.1:1", "m", 8);
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains("embed.provider must be tei|openai"),
        "{err}"
    );
}

#[test]
fn openai_provider_selects_the_data_array_dialect() {
    let server = FakeEmbedServer::spawn_raw(openai_body(8));
    let embedder = build_embedder(
        "openai",
        server.url.clone(),
        "text-embedding-3-small",
        8,
        None,
    )
    .unwrap();
    let out = embedder.embed_batch(&["x".to_owned()]).unwrap();
    assert_eq!(out, vec![vec![0.5_f32; 8]]);
    assert_eq!(embedder.dims(), 8);
}

#[test]
fn unknown_provider_fails_closed_listing_the_allowed_values() {
    assert_eq!(EMBED_PROVIDERS, &["tei", "openai"]);
    let err = err_of("cohere", "http://127.0.0.1:1", "m", 8);
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("tei|openai"), "{err}");
    assert!(err.message.contains("cohere"), "{err}");
}

#[test]
fn adapter_configuration_errors_propagate() {
    assert_eq!(err_of("tei", "", "m", 8).code, ErrorCode::ConfigCorrupt);
    assert_eq!(
        err_of("openai", "http://127.0.0.1:1", "m", 0).code,
        ErrorCode::ConfigCorrupt
    );
}
