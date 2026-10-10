// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for dual-representation plaintext packed before DEK seal.
//!
//! ## Owns
//! - `DualContent` v1 JSON shape (`snapshot` + `gist`)
//! - Serialize/parse of sealed plaintext bytes
//!
//! ## Does not own
//! - DEK encrypt/decrypt
//! - SQL persistence
//!
//! ## Boundary
//!
//! Changes to the sealed dual JSON format should normally require changes here.
//! Changes to store drivers should not require changes here.
//!
//! Snapshot and gist stay separate fields inside the sealed JSON. Plaintext
//! `snapshot_json` / `gist_text` SQL columns remain unused.

use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Versioned plaintext payload sealed into `items.content_ciphertext`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DualContent {
    /// Payload format version.
    pub v: u32,
    /// Authoritative structured half.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Value>,
    /// Non-authoritative prose half.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gist: Option<String>,
    /// Optional bounded descriptive source context.
    ///
    /// Sealed with the content boundary because arbitrary context may contain
    /// personal data; `default` keeps rows sealed before the field existed
    /// readable, and the version stays 1 (additive optional field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl DualContent {
    /// Build a v1 dual payload.
    #[must_use]
    pub fn new(snapshot: Option<Value>, gist: Option<String>, context: Option<String>) -> Self {
        Self {
            v: 1,
            snapshot,
            gist,
            context,
        }
    }

    /// Serialize to bytes for DEK encrypt.
    ///
    /// Panics only if serde JSON encoding of this closed struct fails (should not).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("DualContent serializes")
    }

    /// Parse sealed plaintext bytes.
    ///
    /// # Errors
    /// Malformed JSON or unsupported version. Both are reported as
    /// [`ErrorCode::UnreadableContent`]: the row decrypted, but its bytes are
    /// not readable content. That is the *only* error this function produces,
    /// which is what lets `open_dual` map any failure here to one skippable
    /// classification without inspecting the message.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AmError> {
        let parsed: Self = serde_json::from_slice(bytes).map_err(|e| {
            AmError::new(
                ErrorCode::UnreadableContent,
                format!("dual content parse: {e}"),
            )
        })?;
        if parsed.v != 1 {
            return Err(AmError::new(
                ErrorCode::UnreadableContent,
                format!("unsupported dual content version {}", parsed.v),
            ));
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_and_version_gate() {
        let c = DualContent::new(Some(json!({"a": 1})), Some("g".into()), Some("chat".into()));
        let bytes = c.to_bytes();
        assert_eq!(DualContent::from_bytes(&bytes).unwrap(), c);
        assert!(DualContent::from_bytes(b"{").is_err());
        let bad = br#"{"v":99}"#;
        assert!(DualContent::from_bytes(bad).is_err());
        let empty = DualContent::new(None, None, None);
        assert!(
            DualContent::from_bytes(&empty.to_bytes())
                .unwrap()
                .snapshot
                .is_none()
        );
    }

    #[test]
    fn payload_written_before_context_existed_still_reads() {
        // v1 rows sealed before carry no `context` key at all.
        let legacy = br#"{"v":1,"snapshot":{"a":1},"gist":"g"}"#;
        let parsed = DualContent::from_bytes(legacy).expect("legacy payload");
        assert!(parsed.context.is_none());
        assert_eq!(parsed.gist.as_deref(), Some("g"));
        // Absent context is omitted from new output (no empty-string sentinel).
        let out = serde_json::to_string(&DualContent::new(None, None, None)).unwrap();
        assert!(!out.contains("context"));
    }
}
