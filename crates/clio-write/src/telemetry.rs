// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for verify/refuse telemetry with secret masking.
//!
//! ## Owns
//! - `VerifyEvent` shape and in-memory sink
//! - Diagnostic preview assembly (delegates scrubbing to `clio-config::secret`)
//! - The optional presenting credential label on a verify event
//!
//! ## Does not own
//! - Admission accept/reject events (see `clio-admission`)
//! - Inline scrub algorithm (see `clio-config::secret::scrub_inline_secrets`)
//! - The read-side per-hit `entities[]` projection (see `clio-retrieve`); this
//!   module never carries it. Note the pre-existing masked snapshot preview
//!   does carry the snapshot's own `entity` value: that is write-path
//!   diagnostics (secret masking only), not part of the read-side entity
//!   boundary, and whether the preview may carry a name is a requirement-owner
//!   decision
//!
//! ## Boundary
//!
//! Changes to verify telemetry fields should normally require changes here.

use clio_config::secret::{masked_clone, scrub_inline_secrets};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

/// One verify-path telemetry event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyEvent {
    /// `verify_fail` | `verify_retry` | `verify_refuse` | `verify_ok`.
    pub operation: String,
    /// Bank isolation key.
    pub bank: String,
    /// Attribution actor.
    pub actor: String,
    /// Label of the credential that presented the operation; absent when no
    /// credential did. Never a secret and never a digest of one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// Provenance id (`source_ref`).
    pub source_ref: String,
    /// Caller timestamp.
    pub timestamp: String,
    /// Failed field paths (when applicable).
    pub failure_fields: Vec<String>,
    /// Secret-masked diagnostic preview.
    pub preview: Value,
}

/// Sink for verify telemetry.
pub trait VerifySink: std::fmt::Debug {
    /// Record one event.
    fn emit(&self, event: VerifyEvent);
}

/// In-memory sink for tests and single-process hosts.
#[derive(Debug, Default)]
pub struct VerifyVecSink {
    events: Mutex<Vec<VerifyEvent>>,
}

impl VerifyVecSink {
    /// Snapshot of recorded events.
    #[must_use]
    pub fn events(&self) -> Vec<VerifyEvent> {
        self.events.lock().expect("verify sink poisoned").clone()
    }
}

impl VerifySink for VerifyVecSink {
    fn emit(&self, event: VerifyEvent) {
        self.events
            .lock()
            .expect("verify sink poisoned")
            .push(event);
    }
}

/// Scrub inline secrets and mask JSON trees for diagnostics.
#[must_use]
pub fn redact_preview(
    snapshot: Option<&Value>,
    gist: Option<&str>,
    diagnostic: Option<&Value>,
) -> Value {
    let snap = snapshot.map_or(Value::Null, masked_clone);
    let gist = gist.map(|g| {
        let short: String = g.chars().take(160).collect();
        scrub_inline_secrets(&short)
    });
    let diagnostic = diagnostic.map(masked_clone);
    serde_json::json!({
        "snapshot": snap,
        "gist": gist,
        "diagnostic": diagnostic,
    })
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod telemetry_tests;

#[cfg(test)]
#[path = "telemetry_entity_tests.rs"]
mod telemetry_entity_tests;
