// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing bind admission: the four decision
//! cases, the words of the refusal, and the fact that `serve` refuses a real
//! non-loopback bind while leaving no listener behind.
//!
//! ## Owns
//! - Behavior coverage for `bind_admission`
//!
//! ## Does not own
//! - The loopback classification (`bind_warning_tests`)
//! - The warning text an admitted public bind prints
//!   (`bind_warning_tests`)
//!
//! ## Boundary
//!
//! These tests break when the admission rule or the refusal text changes; they
//! never change it.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::server::{SyncServerOptions, sync_serve};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, Store, SyncStore};
use clio_types::ErrorCode;
use std::net::TcpListener;
use std::sync::Arc;

fn addr(text: &str) -> SocketAddr {
    text.parse().expect("parse test address")
}

/// A loopback bind needs no permission, so it is admitted with the setting in
/// either position.
#[test]
fn loopback_is_admitted_whatever_the_setting_says() {
    assert_eq!(bind_admission(true, false), None);
    assert_eq!(bind_admission(true, true), None);
}

/// A non-loopback bind is admitted only when the operator allows it.
#[test]
fn public_bind_is_admitted_only_with_the_setting_on() {
    assert_eq!(bind_admission(false, true), None);
    assert_eq!(
        bind_admission(false, false),
        Some(BindRefusal::NonLoopbackNotAllowed)
    );
}

/// The refusal has to tell an operator which address was refused and which
/// setting admits it, or the only way out of it is unguessable.
#[test]
fn the_refusal_names_the_address_and_the_setting() {
    let message = refusal_message(addr("0.0.0.0:34311"));
    assert!(
        message.contains("0.0.0.0:34311"),
        "the refusal names the address: {message}"
    );
    assert!(
        message.contains("sync.allow_public_bind"),
        "the refusal names the operator setting: {message}"
    );
    assert!(
        message.contains("not loopback") && message.contains("refused"),
        "the refusal says a non-loopback bind is refused: {message}"
    );
}

/// `serve` binds first and refuses afterwards, so the refusal has to arrive as a
/// `403` naming the setting, and the listener must go with it. The address is a
/// wildcard port taken from a probe listener so the bind is real without
/// depending on a fixed port; the wildcard bind is safe here because this crate
/// holds no privileged interface and the port is ephemeral.
#[test]
fn serve_refuses_a_public_bind_when_the_setting_is_off() {
    let probe = TcpListener::bind("0.0.0.0:0").expect("probe bind");
    let port = probe.local_addr().expect("probe local addr").port();
    drop(probe);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("sqlite open"));
    let err = sync_serve(SyncServerOptions {
        bind: format!("0.0.0.0:{port}"),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: Arc::clone(&store) as Arc<dyn SyncStore>,
        kms,
        banks: vec!["b-a".into()],
        token_file: None,
        dev_mode: true,
        allow_public_bind: false,
        ttl_secs: None,
        deadline_secs: None,
        max_envelope_skew_secs: Some(0),
        device_id: "srv-1".into(),
        sync_key: None,
    })
    .err()
    .expect("a public bind is refused with the setting off");
    assert_eq!(
        err.code,
        ErrorCode::Forbidden,
        "a refused bind is a forbidden start: {}",
        err.message
    );
    assert!(
        err.message.contains("sync.allow_public_bind"),
        "the refusal names the operator setting: {}",
        err.message
    );
    assert!(
        err.message.contains(&format!("0.0.0.0:{port}")),
        "the refusal names the address: {}",
        err.message
    );
    TcpListener::bind(("0.0.0.0", port))
        .expect("the refused bind left nothing listening on the address");
}
