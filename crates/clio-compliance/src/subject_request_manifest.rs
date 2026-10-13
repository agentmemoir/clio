// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the copy-custody manifest field contract:
//! the stable namespaced field keys, the digest and timestamp shapes, the
//! producer and receiver identity fields, and immutable written manifests.
//!
//! ## Owns
//! - `CopyCustodyManifest` and its `write` validation
//! - The `KEY_*` stable namespaced field keys
//! - The `sha256:` digest shape and the RFC 3339 UTC timestamp shape
//!
//! ## Does not own
//! - Action events and their envelope (see `subject_request_events.rs`)
//! - The request state table and ledger (see `subject_request.rs`)
//! - Any storage adapter, SQL, I/O, or copy performed: writing a manifest here
//!   is constructing and validating an in-memory value
//! - Subject content, identity proof, key material, or tokens: no field here
//!   holds them, and identities must stay generic
//!
//! ## Boundary
//!
//! Changes to the field keys, field shapes, or validation rules should
//! normally require changes here. Changes to how a manifest is stored or to
//! event shape should not require changes here.
//!
//! Keep this module focused on the pure manifest field contract.

use clio_types::{AmError, ErrorCode};
use serde::{Deserialize, Serialize};

/// The stable namespaced key of the producer identity field.
pub const KEY_PRODUCER: &str = "com.agentmemoir.custody.producer";

/// The stable namespaced key of the receiver identity field.
pub const KEY_RECEIVER: &str = "com.agentmemoir.custody.receiver";

/// The stable namespaced key of the copy digest field.
pub const KEY_COPY_DIGEST: &str = "com.agentmemoir.custody.copy_digest";

/// The stable namespaced key of the produced-at timestamp field.
pub const KEY_PRODUCED_AT: &str = "com.agentmemoir.custody.produced_at";

/// The stable namespaced key of the received-at timestamp field.
pub const KEY_RECEIVED_AT: &str = "com.agentmemoir.custody.received_at";

/// The required digest algorithm prefix.
pub const DIGEST_ALGORITHM_PREFIX: &str = "sha256:";

/// A written copy-custody manifest: the custody fields of one subject copy.
///
/// Treat a written manifest as immutable: [`CopyCustodyManifest::write`]
/// validates every field and no method mutates a written manifest, so callers
/// must not modify its fields after writing. Field values stay generic: a
/// manifest never carries subject content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyCustodyManifest {
    /// The identity of the producer that wrote the copy.
    pub producer: String,
    /// The identity of the receiver that holds the copy.
    pub receiver: String,
    /// The copy digest, prefixed with its algorithm (`sha256:<hex>`).
    pub copy_digest: String,
    /// When the copy was produced (RFC 3339 UTC).
    pub produced_at: String,
    /// When the copy was received (RFC 3339 UTC).
    pub received_at: String,
}

impl CopyCustodyManifest {
    /// Write a manifest.
    ///
    /// Every field is validated: identities must not be blank, the digest must
    /// carry its algorithm prefix, and both timestamps must be RFC 3339 UTC.
    /// On success treat the manifest as immutable: no method mutates it.
    pub fn write(
        producer: &str,
        receiver: &str,
        copy_digest: &str,
        produced_at: &str,
        received_at: &str,
    ) -> Result<Self, AmError> {
        if producer.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "manifest producer identity must not be blank",
            ));
        }
        if receiver.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "manifest receiver identity must not be blank",
            ));
        }
        if !is_sha256_digest(copy_digest) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "manifest copy digest must be a sha256-prefixed hex digest",
            ));
        }
        if !is_rfc3339_utc(produced_at) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "manifest produced-at must be an RFC 3339 UTC timestamp",
            ));
        }
        if !is_rfc3339_utc(received_at) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "manifest received-at must be an RFC 3339 UTC timestamp",
            ));
        }
        Ok(Self {
            producer: producer.to_owned(),
            receiver: receiver.to_owned(),
            copy_digest: copy_digest.to_owned(),
            produced_at: produced_at.to_owned(),
            received_at: received_at.to_owned(),
        })
    }

    /// The value of a manifest field by its stable namespaced key.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&str> {
        match key {
            KEY_PRODUCER => Some(&self.producer),
            KEY_RECEIVER => Some(&self.receiver),
            KEY_COPY_DIGEST => Some(&self.copy_digest),
            KEY_PRODUCED_AT => Some(&self.produced_at),
            KEY_RECEIVED_AT => Some(&self.received_at),
            _ => None,
        }
    }
}

/// The shape check for a `sha256:`-prefixed hex digest. Not a content check.
fn is_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(DIGEST_ALGORITHM_PREFIX) else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte: u8| byte.is_ascii_hexdigit())
}

/// The shape check for an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`,
/// with an optional fractional-second part). Not a full parse.
pub(crate) fn is_rfc3339_utc(timestamp: &str) -> bool {
    let bytes = timestamp.as_bytes();
    if bytes.len() < 20
        || !bytes[..19]
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                10 => *byte == b'T',
                13 | 16 => *byte == b':',
                _ => byte.is_ascii_digit(),
            })
    {
        return false;
    }
    match bytes[19] {
        b'Z' => bytes.len() == 20,
        b'.' => {
            bytes.len() >= 22
                && bytes[20..bytes.len() - 1].iter().all(u8::is_ascii_digit)
                && bytes[bytes.len() - 1] == b'Z'
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIME: &str = "2026-01-01T00:00:00Z";
    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn written() -> CopyCustodyManifest {
        CopyCustodyManifest::write("producer-a", "receiver-b", DIGEST, TIME, TIME).unwrap()
    }

    #[test]
    fn write_refuses_blank_identities() {
        let err = CopyCustodyManifest::write("  ", "r", DIGEST, TIME, TIME).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        let err = CopyCustodyManifest::write("p", "", DIGEST, TIME, TIME).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn write_refuses_digests_without_the_algorithm_prefix_or_hex_shape() {
        for bad in [
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "md5:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "sha256:0123456789abcdef",
            "sha256:0123456789abcdefg0123456789abcdef0123456789abcdef0123456789a",
            "sha256:gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0",
            "",
        ] {
            let err = CopyCustodyManifest::write("p", "r", bad, TIME, TIME).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidArgument, "{bad}");
        }
    }

    #[test]
    fn write_refuses_timestamps_that_are_not_rfc3339_utc() {
        for bad in [
            "2026-01-01 00:00:00Z",
            "2026-01-01T00:00:00",
            "2026-13-01T00:00:00Zx",
            "2026-01-01T00:00:00.",
        ] {
            let err = CopyCustodyManifest::write("p", "r", DIGEST, bad, TIME).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidArgument, "{bad}");
            let err = CopyCustodyManifest::write("p", "r", DIGEST, TIME, bad).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidArgument, "{bad}");
        }
    }

    #[test]
    fn write_accepts_distinct_produced_and_received_timestamps() {
        let manifest =
            CopyCustodyManifest::write("p", "r", DIGEST, TIME, "2026-01-02T00:00:00Z").unwrap();
        assert_eq!(manifest.field(KEY_PRODUCED_AT), Some(TIME));
        assert_eq!(
            manifest.field(KEY_RECEIVED_AT),
            Some("2026-01-02T00:00:00Z")
        );
    }

    #[test]
    fn the_timestamp_check_is_shape_only_and_accepts_impossible_dates() {
        // The checker verifies positions and digits, not the calendar: month
        // 13 passes. A calendar-aware check is a later contract change.
        CopyCustodyManifest::write("p", "r", DIGEST, "2026-13-01T00:00:00Z", TIME).unwrap();
    }

    #[test]
    fn write_accepts_fractional_second_timestamps() {
        CopyCustodyManifest::write("p", "r", DIGEST, "2026-01-01T00:00:00.123Z", TIME).unwrap();
    }

    #[test]
    fn field_maps_the_stable_namespaced_keys() {
        let manifest = written();
        assert_eq!(manifest.field(KEY_PRODUCER), Some("producer-a"));
        assert_eq!(manifest.field(KEY_RECEIVER), Some("receiver-b"));
        assert_eq!(manifest.field(KEY_COPY_DIGEST), Some(DIGEST));
        assert_eq!(manifest.field(KEY_PRODUCED_AT), Some(TIME));
        assert_eq!(manifest.field(KEY_RECEIVED_AT), Some(TIME));
        assert_eq!(manifest.field("com.agentmemoir.custody.unknown"), None);
    }

    #[test]
    fn a_written_manifest_survives_a_serde_roundtrip_unchanged() {
        let bytes = serde_json::to_string(&written()).expect("serialize manifest");
        let reloaded: CopyCustodyManifest =
            serde_json::from_str(&bytes).expect("deserialize manifest");
        assert_eq!(reloaded, written());
    }
}
