// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for ops embedder tests: provider selection
//! (an omitted provider resolves to the tei sidecar default, an explicitly
//! empty provider fails closed), the tei and openai factory routes, and the
//! fail-closed configuration rules (missing model, zero dims, unknown
//! provider, unconfigured url).
//!
//! ## Owns
//!
//! - `OpsEmbedConfig::from_effective` field and default assertions
//! - `build_ops_embedder_from` selection and fail-closed assertions
//!
//! ## Does not own
//!
//! - Runtime wiring of the built embedder (`runtime_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{OpsEmbedConfig, build_ops_embedder_from};
use clio_types::ErrorCode;
use serde_json::json;

/// A local sidecar address; the factory only builds adapters and never dials.
const SIDECAR: &str = "http://127.0.0.1:34311";

fn cfg(provider: &str, url: &str, model: &str, dims: usize) -> OpsEmbedConfig {
    OpsEmbedConfig {
        provider: provider.into(),
        url: url.into(),
        model: model.into(),
        dims,
        bearer: None,
    }
}

#[test]
fn empty_url_disables_live_dense_reindex() {
    assert!(
        build_ops_embedder_from(&cfg("", "", "", 384))
            .unwrap()
            .is_none()
    );
    assert!(
        build_ops_embedder_from(&cfg("", "   ", "", 384))
            .unwrap()
            .is_none()
    );
}

#[test]
fn tei_provider_builds_sidecar_route() {
    // The tei provider may use the sidecar's conventional default model.
    assert!(
        build_ops_embedder_from(&cfg("tei", SIDECAR, "", 384))
            .unwrap()
            .is_some()
    );
    assert!(
        build_ops_embedder_from(&cfg("tei", SIDECAR, "custom-model", 384))
            .unwrap()
            .is_some()
    );
}

#[test]
fn openai_provider_builds_hosted_route() {
    assert!(
        build_ops_embedder_from(&cfg("openai", SIDECAR, "text-embedding-3-small", 1536))
            .unwrap()
            .is_some()
    );
}

#[test]
fn explicit_empty_provider_fails_closed() {
    // An explicitly empty provider is not the unset default; it fails closed.
    assert!(build_ops_embedder_from(&cfg("", SIDECAR, "", 384)).is_err());
}

#[test]
fn omitted_provider_resolves_to_tei_default() {
    let unset = json!({ "embed": { "url": SIDECAR, "dims": 384 } });
    let parsed = OpsEmbedConfig::from_effective(&unset, None);
    assert_eq!(parsed.provider, "tei");
    assert!(build_ops_embedder_from(&parsed).unwrap().is_some());
}

#[test]
fn unknown_provider_fails_closed_with_allowed_values() {
    let Err(err) = build_ops_embedder_from(&cfg("cohere", SIDECAR, "m", 384)) else {
        panic!("unknown provider must fail closed");
    };
    assert!(err.message.contains("tei|openai"), "{err}");
}

#[test]
fn rejected_endpoint_value_errors_rather_than_disabling() {
    assert!(build_ops_embedder_from(&cfg("", "ftp://nope", "", 384)).is_err());
}

#[test]
fn missing_model_fails_closed_for_non_tei_provider() {
    let Err(err) = build_ops_embedder_from(&cfg("openai", SIDECAR, "", 384)) else {
        panic!("missing embed.model with a configured provider must fail closed");
    };
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    assert!(err.message.contains("embed.model"), "{err}");
}

#[test]
fn zero_dims_fails_closed() {
    let Err(err) = build_ops_embedder_from(&cfg("tei", SIDECAR, "", 0)) else {
        panic!("zero embed.dims with a configured url must fail closed");
    };
    assert_eq!(err.code, ErrorCode::ConfigCorrupt, "{err}");
    assert!(err.message.contains("embed.dims"), "{err}");
}

#[test]
fn from_effective_reads_embed_fields() {
    let effective = json!({
        "embed": {
            "url": SIDECAR,
            "model": "local/nomic-embed-text",
            "dims": 768,
            "provider": "openai"
        }
    });
    let parsed = OpsEmbedConfig::from_effective(&effective, Some("key".into()));
    assert_eq!(parsed.provider, "openai");
    assert_eq!(parsed.dims, 768);
    assert_eq!(parsed.model, "local/nomic-embed-text");
    assert_eq!(parsed.bearer.as_deref(), Some("key"));
    assert!(build_ops_embedder_from(&parsed).unwrap().is_some());
}

#[test]
fn from_effective_defaults_to_unconfigured() {
    let empty = OpsEmbedConfig::from_effective(&json!({}), None);
    assert!(empty.url.is_empty(), "{:?}", empty.url);
    assert_eq!(empty.dims, clio_store::EMBEDDING_DIMS);
    assert!(build_ops_embedder_from(&empty).unwrap().is_none());
}
