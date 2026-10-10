// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for content-hash mission rule ids: the FNV-1a 64
//! hash over normalized entry text and the match discriminator.
//!
//! ## Owns
//! - `mission_rule_id` and its inputs (`mission_rule_key`, NFC normalization, FNV-1a 64)
//!
//! ## Does not own
//! - Mission validation and duplicate rejection (see `retention_mission`)
//! - Pattern matching internals (see `mission_match`)
//!
//! ## Boundary
//!
//! Changes to the id recipe (normalization, separator, discriminator, hash)
//! should normally require changes here. Changes to validation should not.
//!
//! Keep this module focused on mission rule id derivation.

use crate::retention_mission::MissionEntry;
use unicode_normalization::UnicodeNormalization;

/// Stable duplicate key for one entry: normalized text plus discriminator.
pub(crate) fn mission_rule_key(entry: &MissionEntry) -> String {
    format!(
        "{}\u{1}{}",
        normalized_entry_text(&entry.text),
        entry.match_kind.as_str()
    )
}

fn normalized_entry_text(text: &str) -> String {
    text.nfc().flat_map(char::to_lowercase).collect()
}

/// FNV-1a 64 over bytes (same family as the tool-catalog hash).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Content-hash rule id for one entry: lowercase NFC-normalized entry text
/// concatenated with the `match` discriminator, hex-encoded. Ids are stable
/// across mission edits that do not touch the entry.
#[must_use]
pub fn mission_rule_id(entry: &MissionEntry) -> String {
    let hash = fnv1a64(mission_rule_key(entry).as_bytes());
    format!("{hash:016x}")
}
