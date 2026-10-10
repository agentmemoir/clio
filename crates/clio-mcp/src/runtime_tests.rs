// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for runtime wiring tests: open options, bank and
//! actor precedence, signal sources, extractor configuration, and key
//! persistence across state opens. Stderr sink serialization tests live in
//! `runtime_sink_tests`, timestamp resolution tests in `runtime_stamp_tests`,
//! and ops embedder tests in `ops_embedder_tests`.
//!
//! ## Owns
//! - `McpState::open` behaviour over both backends, including bank/actor
//!   argument precedence and next-item-id allocation
//! - Empty signal sources, extractor fail-closed configuration, and
//!   file-backed KMS persistence across opens
//!
//! ## Does not own
//! - Stderr sink serialization (`runtime_sink_tests`)
//! - Timestamp resolution (`runtime_stamp_tests`)
//! - Ops embedder provider selection and fail-closed rules
//!   (`ops_embedder_tests`)
//!
//! ## Boundary
//!
//! Tests here break when runtime wiring changes; they never change it.
//!
//! Keep this module focused on runtime wiring.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_admission::gated::SignalSource;
use clio_belief::signals::BeliefSignalSource;
use serde_json::json;
use std::sync::Arc;

pub(crate) fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[test]
fn opens_sqlite_in_memory() {
    let s = state();
    assert_eq!(s.default_bank, "default");
    assert_eq!(s.default_actor, clio_types::Actor::Agent);
    assert!(s.hub.is_some());
}

#[test]
fn unknown_backend_fails_with_out_of_range() {
    let err = McpState::open(&McpOpenOptions {
        backend: "mysql".into(),
        ..McpOpenOptions::default()
    })
    .expect_err("unknown backend");
    assert_eq!(err.code, clio_types::ErrorCode::OutOfRange);
}

#[test]
fn bank_and_actor_args_win_over_defaults() {
    let s = McpState::open(&McpOpenOptions {
        default_bank: "default".into(),
        default_actor: clio_types::Actor::System,
        ..McpOpenOptions::default()
    })
    .expect("state");
    let ctx = s
        .resolve_ctx(&json!({"bank": "bank-x", "actor": "user"}))
        .expect("ctx");
    assert_eq!(ctx.bank.as_str(), "bank-x");
    assert_eq!(ctx.actor.as_str(), "user");
    let fallback = s.resolve_ctx(&json!({})).expect("ctx");
    assert_eq!(fallback.bank.as_str(), "default");
    assert_eq!(fallback.actor.as_str(), "system");
}

#[test]
fn empty_resolved_bank_rejected() {
    let s = state();
    let err = s.resolve_ctx(&json!({"bank": ""})).expect_err("empty bank");
    assert_eq!(err.code, clio_types::ErrorCode::MissingBank);
    let err = s
        .resolve_ctx(&json!({"actor": "robot"}))
        .expect_err("bad actor");
    assert_eq!(err.code, clio_types::ErrorCode::InvalidArgument);
}

#[test]
fn next_item_id_increments() {
    let s = state();
    let a = s.next_item_id();
    let b = s.next_item_id();
    assert_ne!(a, b);
    assert!(a.starts_with("itm-mcp-"));
}

#[test]
fn no_signals_are_empty() {
    let s = NoSignals;
    let item = test_item();
    let neighbors = SignalSource::neighbor_texts(&s, &item).expect("neighbors");
    assert!(neighbors.is_empty(), "{neighbors:?}");
    assert!(
        SignalSource::prior_age_days(&s, &item)
            .expect("age")
            .is_none()
    );
    let belief_neighbors = BeliefSignalSource::neighbor_texts(&s, "b", "p").expect("neighbors");
    assert!(belief_neighbors.is_empty(), "{belief_neighbors:?}");
    assert!(
        BeliefSignalSource::prior_age_days(&s, "b", "p")
            .expect("age")
            .is_none()
    );
}

fn test_item() -> clio_types::MemoryItem {
    clio_types::MemoryItem {
        id: "i".into(),
        bank_id: "b".into(),
        subject_id: "s".into(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: None,
        valid_from: "t".into(),
        valid_until: None,
        tx_from: "t".into(),
        tx_until: None,
        created_at: "t".into(),
        updated_at: "t".into(),
        context: None,
    }
}

#[test]
fn mcp_state_debug_formats() {
    let s = state();
    let text = format!("{s:?}");
    assert!(text.contains("McpState"));
    assert!(text.contains("default"));
}

#[test]
fn opens_postgres_when_database_url_configured() {
    let Ok(url) = std::env::var("CLIO_DATABASE_URL") else {
        return;
    };
    if !url.starts_with("postgres") {
        return;
    }
    // Serialize against clio-store's shared-Postgres tests:
    // this open migrates the same shared database they delete rows from.
    let _guard = clio_store::lock_pg_test();
    let opened = McpState::open(&McpOpenOptions {
        backend: "postgres".into(),
        database_url: url,
        ..McpOpenOptions::default()
    });
    assert!(opened.is_ok(), "postgres open failed: {:?}", opened.err());
}

/// Extraction readiness mirrors the fail-closed rules of
/// `clio_write::build_extractor_from`: hosted openai needs model + credential,
/// unknown providers never report ready (truthful readiness).
#[test]
fn extract_available_matches_ingest_fail_closed() {
    let ready = |effective: Value| {
        McpState::open_with_effective(&McpOpenOptions::default(), &effective)
            .expect("state")
            .pipeline
            .extract_available
    };
    let hosted = |url: &str, model: &str, key: Option<&str>| {
        let mut effective = json!({"extract": {"provider": "openai", "url": url, "model": model}});
        if let Some(k) = key {
            effective["credentials"] = json!({"extract_api_key": k});
        }
        effective
    };
    assert!(
        !ready(hosted("http://x", "m", None)),
        "openai without a key"
    );
    assert!(
        !ready(hosted("http://x", "", Some("k"))),
        "openai without a model"
    );
    assert!(
        ready(hosted("http://x", "m", Some("k"))),
        "fully configured hosted"
    );
    assert!(
        ready(json!({"extract": {"url": "http://x"}})),
        "template default"
    );
    assert!(
        !ready(json!({"extract": {"url": "", "model": "m"}})),
        "no url"
    );
    let unknown = json!({"extract": {"provider": "cohere", "url": "http://x"}});
    assert!(!ready(unknown), "unknown provider");
}

#[test]
fn file_backed_kms_persists_across_state_opens() {
    // Hermetic sealing: the process test key, never the machine's key store.
    clio_config::secret_seal::test_key::install_process_key(
        [0x6b; clio_config::secret_key::KEY_LEN],
    );
    // Regression for #2 at the runtime seam: two `McpState::open` calls over the
    // same database and key-store path share DEKs, so content written by the
    // first is readable by the second.
    let dir = std::env::temp_dir().join(format!("clio-runtime-kms-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let opts = McpOpenOptions {
        backend: "sqlite".into(),
        database_url: dir.join("clio.db").to_string_lossy().into_owned(),
        kms_path: Some(dir.join("clio.kms.json")),
        ..McpOpenOptions::default()
    };
    let first = McpState::open(&opts).expect("first open");
    let envelope = first
        .kms
        .encrypt("sub-runtime", b"persist")
        .expect("encrypt");
    drop(first);
    let second = McpState::open(&opts).expect("second open");
    assert_eq!(second.kms.decrypt(&envelope).expect("decrypt"), b"persist");
    let _ = std::fs::remove_dir_all(&dir);
}
