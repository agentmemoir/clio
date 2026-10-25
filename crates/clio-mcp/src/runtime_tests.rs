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
//! - Hermetic `McpState::open_with_effective` behaviour over both backends,
//!   including bank/actor argument precedence, next-item-id allocation, and
//!   the document-named erasure-journal seam
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
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// The hermetic constructor must ignore the operator's deployment file, while
/// the deployment path layers it. This guards every test that asserts "no
/// sidecar configured": on a host with a configured deployment that assertion
/// was about the machine, not the code.
#[test]
fn hermetic_open_ignores_the_deployment_file() {
    let dir = std::env::temp_dir().join(format!("clio-runtime-overlay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("deployment.json");
    std::fs::write(
        &path,
        r#"{"rerank":{"provider":"tei","url":"http://127.0.0.1:1","model":"m"}}"#,
    )
    .expect("write overlay");

    let layered = McpState::open_with_runtime(
        &McpOpenOptions::default(),
        clio_config::Runtime::with_env_and_deployment_file(&|_| None, path),
    )
    .expect("layered open");
    assert!(
        layered.pipeline.rerank_active,
        "the deployment overlay must attach the reranker"
    );

    let hermetic =
        McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
            .expect("hermetic open");
    assert!(
        !hermetic.pipeline.rerank_active,
        "the hermetic open must ignore the deployment file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The document seam keeps only a usable string: a named path survives
/// (trimmed), while an absent key, a blank or NUL-bearing value, or a
/// non-string names nothing.
#[test]
fn erasure_journal_path_from_document_keeps_only_usable_paths() {
    let named = json!({"erasure_journal_file": "  /tmp/journal.json  "});
    assert_eq!(
        erasure_journal_path_from_document(&named),
        Some(std::path::PathBuf::from("/tmp/journal.json"))
    );
    for unnamed in [
        json!({}),
        json!({"erasure_journal_file": ""}),
        json!({"erasure_journal_file": "   "}),
        json!({"erasure_journal_file": "bad\u{0}path"}),
        json!({"erasure_journal_file": 7}),
    ] {
        assert_eq!(
            erasure_journal_path_from_document(&unnamed),
            None,
            "{unnamed}"
        );
    }
}

/// A host document that names the journal path sets the state field; a
/// document without the key leaves it to the deployment resolution.
#[test]
fn open_with_effective_takes_the_document_named_journal_path() {
    let path = std::env::temp_dir().join(format!("clio-runtime-journal-{}", std::process::id()));
    let named = McpState::open_with_effective(
        &McpOpenOptions::default(),
        &json!({"erasure_journal_file": path.to_string_lossy()}),
    )
    .expect("state");
    assert_eq!(named.journal_path.as_deref(), Some(path.as_path()));

    let unnamed =
        McpState::open_with_effective(&McpOpenOptions::default(), &json!({})).expect("state");
    assert_eq!(unnamed.journal_path, None);
}

/// The deployment constructor never takes the document key, even when a
/// runtime document carries one, so `open` keeps the environment resolution.
#[test]
fn open_with_runtime_keeps_the_ambient_journal_resolution() {
    let mut runtime = clio_config::Runtime::with_env(&|_| None);
    runtime
        .load_file_json(r#"{"erasure_journal_file": "/tmp/named-by-the-document.json"}"#)
        .expect("file layer");
    assert_eq!(
        runtime
            .effective_raw()
            .pointer("/erasure_journal_file")
            .and_then(Value::as_str),
        Some("/tmp/named-by-the-document.json"),
        "premise: the document carries the key"
    );
    let state = McpState::open_with_runtime(&McpOpenOptions::default(), runtime).expect("state");
    assert_eq!(state.journal_path, None);
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
    let err = McpState::open_with_effective(
        &McpOpenOptions {
            backend: "mysql".into(),
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
    .expect_err("unknown backend");
    assert_eq!(err.code, clio_types::ErrorCode::OutOfRange);
}

#[test]
fn bank_and_actor_args_win_over_defaults() {
    let s = McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: "default".into(),
            default_actor: clio_types::Actor::System,
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    )
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
    let belief_neighbors = BeliefSignalSource::neighbor_texts(&s, "b", "p").expect("neighbors");
    assert!(belief_neighbors.is_empty(), "{belief_neighbors:?}");
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
    let opened = McpState::open_with_effective(
        &McpOpenOptions {
            backend: "postgres".into(),
            database_url: url,
            ..McpOpenOptions::default()
        },
        &serde_json::json!({}),
    );
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

/// A request-scoped copy differs only in who it acts as: the credential lands on
/// the copy, the original stays unbound, and every shared field is still the one
/// instance because each already sits behind an `Arc`.
#[test]
fn with_principal_scopes_one_copy_of_the_state() {
    let base = state();
    let scoped = base.with_principal(Principal::new(
        "ops-key".to_owned(),
        Actor::Harness,
        vec!["bank-a".to_owned()],
    ));
    let attached = scoped
        .principal
        .as_deref()
        .expect("the copy carries the credential");
    assert_eq!(
        attached.label, "ops-key",
        "the copy acts as the named record"
    );
    assert_eq!(
        attached.actor,
        Actor::Harness,
        "the copy acts as the credential's actor"
    );
    assert!(
        Arc::ptr_eq(&scoped.store, &base.store),
        "a shallow copy shares one store instead of forking it"
    );
    assert!(base.principal.is_none(), "the original stays unbound");
}

/// An unbound state administers: a local transport has no credential edge, and
/// the process owner already holds the store.
#[test]
fn authorize_admin_allows_an_unbound_state() {
    let base = state();
    assert!(
        base.principal.is_none(),
        "premise: the helper opens unbound"
    );
    base.authorize_admin()
        .expect("a local, unbound state administers");
}

/// Only `harness` and `system` carry the privilege, so an `agent` or `user`
/// credential is refused — and the refusal is the code, not the message text.
#[test]
fn authorize_admin_refuses_a_non_administrative_credential() {
    for actor in [Actor::Agent, Actor::User] {
        let bound = state().with_principal(Principal::new(
            format!("{}-key", actor.as_str()),
            actor,
            vec!["default".to_owned()],
        ));
        let err = bound
            .authorize_admin()
            .expect_err("a non-administrative credential must not administer");
        assert_eq!(err.code, ErrorCode::Forbidden, "{actor:?} credential");
    }
}

/// An administrative credential administers; `with_principal` is the only way a
/// state becomes bound, so this pins the allow side of the same match arm.
#[test]
fn authorize_admin_allows_an_administrative_credential() {
    for actor in [Actor::Harness, Actor::System] {
        state()
            .with_principal(Principal::new(
                format!("{}-key", actor.as_str()),
                actor,
                vec!["default".to_owned()],
            ))
            .authorize_admin()
            .expect("an administrative credential administers");
    }
}

/// The deployment constructor opens over the ambient configuration and the
/// default in-memory backend, and it leaves the state unbound.
#[test]
fn open_reads_the_deployment_runtime() {
    let opened = McpState::open(&McpOpenOptions::default()).expect("deployment open");
    assert_eq!(opened.default_bank, "default");
    assert!(
        opened.principal.is_none(),
        "a deployment state opens unbound"
    );
}

/// A one-shot process joins its `consolidate` wave instead of queueing it,
/// because a detached wave would die with the process; the long-lived
/// constructor keeps the queued default.
#[test]
fn open_one_shot_joins_the_consolidate_wave() {
    let one_shot = McpState::open_one_shot(&McpOpenOptions::default()).expect("one-shot open");
    assert!(
        one_shot.join_consolidate_wave,
        "a one-shot process joins its wave"
    );
    assert!(
        !state().join_consolidate_wave,
        "the long-lived open queues its wave"
    );
}

#[test]
fn file_backed_kms_persists_across_state_opens() {
    // Hermetic sealing: the process test key, never the machine's key store.
    clio_config::secret_seal::test_key::install_process_key(
        [0x6b; clio_config::secret_key::KEY_LEN],
    );
    // Regression for #2 at the runtime seam: two hermetic opens over the same
    // database and key-store path share DEKs, so content written by the first is
    // readable by the second.
    let dir = std::env::temp_dir().join(format!("clio-runtime-kms-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let opts = McpOpenOptions {
        backend: "sqlite".into(),
        database_url: dir.join("clio.db").to_string_lossy().into_owned(),
        kms_path: Some(dir.join("clio.kms.json")),
        ..McpOpenOptions::default()
    };
    let first = McpState::open_with_effective(&opts, &serde_json::json!({})).expect("first open");
    let envelope = first
        .kms
        .encrypt("sub-runtime", b"persist")
        .expect("encrypt");
    drop(first);
    let second = McpState::open_with_effective(&opts, &serde_json::json!({})).expect("second open");
    assert_eq!(second.kms.decrypt(&envelope).expect("decrypt"), b"persist");
    let _ = std::fs::remove_dir_all(&dir);
}
