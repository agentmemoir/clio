// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the ephemeral scratchpad workspace
//! A size-bounded, in-memory, session-keyed transient
//! buffer for intermediate agent reasoning.
//!
//! ## Owns
//! - `Scratchpad` in-memory store keyed by `(session, bank, key)`
//! - Byte/key quota enforcement and TTL eviction (lazy, on access)
//! - Fail-closed quota rejections (`QuotaExceeded`)
//!
//! ## Does not own
//! - Long-term stores: contents are NEVER persisted, indexed, or injected
//!   into retrieval domains (enforced by construction: this
//!   module has no store dependency at all)
//! - MCP binding and tool schemas (`clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to scratchpad capacity, TTL, or key operations should normally
//! require changes here. Changes to MCP binding should not require changes here.
//!
//! Keep this module focused on the ephemeral scratchpad.

use clio_types::{AmError, ErrorCode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Hard capacity per session scratchpad: 64 KB total content.
pub const MAX_TOTAL_BYTES: usize = 64 * 1024;
/// Maximum keys per session scratchpad.
pub const MAX_KEYS: usize = 100;
/// Inactivity TTL before a session scratchpad is evicted (4 hours).
pub const TTL: Duration = Duration::from_hours(4);

/// Per-`(session, bank)` scratchpad state.
#[derive(Debug)]
struct SessionPad {
    entries: HashMap<String, (String, Instant)>,
    total_bytes: usize,
    last_touch: Instant,
}

impl SessionPad {
    fn touch(&mut self, now: Instant) {
        self.last_touch = now;
    }
}

impl Default for SessionPad {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            last_touch: Instant::now(),
        }
    }
}

/// In-memory ephemeral scratchpad (never durable, never indexed).
///
/// The store holds no reference to any backend: contents live only in process
/// memory and vanish on session exit or process termination (documented
/// limitation — nothing here survives a restart by design).
#[derive(Debug)]
pub struct Scratchpad {
    sessions: Mutex<HashMap<(String, String), SessionPad>>,
    ttl: Duration,
}

impl Default for Scratchpad {
    /// Empty scratchpad with the default TTL.
    fn default() -> Self {
        Self::new()
    }
}

impl Scratchpad {
    /// Empty scratchpad with the default TTL.
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(TTL)
    }

    /// Empty scratchpad with an explicit TTL (test / deployment override).
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    fn evict_expired(&self, sessions: &mut HashMap<(String, String), SessionPad>, now: Instant) {
        sessions.retain(|_, pad| now.duration_since(pad.last_touch) < self.ttl);
    }

    /// Store `content` under `key`; overwrites an existing key.
    ///
    /// # Errors
    /// Empty session/bank/key, or a quota rejection (`QuotaExceeded`) when the
    /// write would exceed the 64 KB total or 100-key caps.
    pub fn write(
        &self,
        session: &str,
        bank: &str,
        key: &str,
        content: &str,
        now: Instant,
    ) -> Result<(), AmError> {
        if session.trim().is_empty() || bank.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::MissingBank,
                "session and bank are required for scratchpad writes",
            ));
        }
        if key.trim().is_empty() {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "scratchpad key is required",
            ));
        }
        let mut sessions = self.sessions.lock().expect("scratchpad poisoned");
        self.evict_expired(&mut sessions, now);
        let pad = sessions
            .entry((session.to_owned(), bank.to_owned()))
            .or_default();
        pad.touch(now);
        let bytes = content.len();
        let prior = pad.entries.get(key).map_or(0, |(c, _)| c.len());
        let next_total = pad.total_bytes - prior + bytes;
        if next_total > MAX_TOTAL_BYTES {
            return Err(AmError::new(
                ErrorCode::QuotaExceeded,
                format!(
                    "QUOTA_EXCEEDED: scratchpad write of {bytes} bytes would exceed the \
                     {MAX_TOTAL_BYTES}-byte session cap"
                ),
            ));
        }
        if !pad.entries.contains_key(key) && pad.entries.len() >= MAX_KEYS {
            return Err(AmError::new(
                ErrorCode::QuotaExceeded,
                format!("QUOTA_EXCEEDED: scratchpad write would exceed the {MAX_KEYS}-key cap"),
            ));
        }
        pad.total_bytes = next_total;
        pad.entries
            .insert(key.to_owned(), (content.to_owned(), now));
        Ok(())
    }

    /// Read one key (or the whole session scratchpad when `key` is `None`).
    ///
    /// # Errors
    /// `ItemNotFound` for a missing key with session/bank scope.
    pub fn read(
        &self,
        session: &str,
        bank: &str,
        key: Option<&str>,
        now: Instant,
    ) -> Result<Value, AmError> {
        let mut sessions = self.sessions.lock().expect("scratchpad poisoned");
        self.evict_expired(&mut sessions, now);
        let Some(pad) = sessions.get_mut(&(session.to_owned(), bank.to_owned())) else {
            return match key {
                Some(k) => Err(key_not_found(k)),
                None => Ok(serde_json::json!({
                    "bank": bank,
                    "session": session,
                    "entries": [],
                    "total_bytes": 0
                })),
            };
        };
        pad.touch(now);
        match key {
            Some(k) => {
                let Some((content, _)) = pad.entries.get(k) else {
                    return Err(key_not_found(k));
                };
                Ok(serde_json::json!({
                    "key": k,
                    "content": parse_or_string(content)
                }))
            }
            None => Ok(serde_json::json!({
                "bank": bank,
                "session": session,
                "entries": pad
                    .entries
                    .iter()
                    .map(|(k, (c, _))| serde_json::json!({
                        "key": k,
                        "content": parse_or_string(c)
                    }))
                    .collect::<Vec<_>>(),
                "total_bytes": pad.total_bytes
            })),
        }
    }

    /// Remove one key, or purge the whole session scratchpad when `key` is `None`.
    ///
    /// # Errors
    /// `ItemNotFound` when `key` names a key this session and bank do not hold.
    /// A clear that finds nothing to remove says so rather than reporting success.
    pub fn clear(
        &self,
        session: &str,
        bank: &str,
        key: Option<&str>,
        now: Instant,
    ) -> Result<(), AmError> {
        let mut sessions = self.sessions.lock().expect("scratchpad poisoned");
        self.evict_expired(&mut sessions, now);
        let Some(pad) = sessions.get_mut(&(session.to_owned(), bank.to_owned())) else {
            return key.map_or(Ok(()), |k| Err(key_not_found(k)));
        };
        pad.touch(now);
        if let Some(k) = key {
            let Some((removed, _)) = pad.entries.remove(k) else {
                return Err(key_not_found(k));
            };
            pad.total_bytes = pad.total_bytes.saturating_sub(removed.len());
        } else {
            pad.entries.clear();
            pad.total_bytes = 0;
        }
        Ok(())
    }
}

/// The error for a key this session and bank do not hold.
fn key_not_found(key: &str) -> AmError {
    AmError::new(
        ErrorCode::ItemNotFound,
        format!("scratchpad key `{key}` not found (session/bank scope)"),
    )
}

/// Parse JSON content when possible, else keep it as a plain string.
fn parse_or_string(content: &str) -> Value {
    serde_json::from_str(content).unwrap_or_else(|_| Value::String(content.to_owned()))
}

#[cfg(test)]
#[path = "scratchpad_tests.rs"]
mod scratchpad_tests;
