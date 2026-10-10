// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the ephemeral scratchpad:
//! byte/key capacity limits, session and bank isolation, and TTL eviction
//! The capacity, isolation and TTL contract those tests exist to pin.
//!
//! ## Owns
//! - Capacity, overwrite, isolation, clear, and TTL assertions for `Scratchpad`
//!
//! ## Does not own
//! - The scratchpad implementation itself (see `scratchpad.rs`)
//! - MCP tool binding for `scratchpad_*` (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{MAX_KEYS, MAX_TOTAL_BYTES, Scratchpad};
use clio_types::ErrorCode;
use std::time::{Duration, Instant};

fn now() -> Instant {
    Instant::now()
}

#[test]
fn write_read_round_trip_and_overwrite() {
    let pad = Scratchpad::new();
    pad.write("s1", "b1", "k1", r#"{"a":1}"#, now()).unwrap();
    let read = pad.read("s1", "b1", Some("k1"), now()).unwrap();
    assert_eq!(read["key"], "k1");
    assert_eq!(read["content"]["a"], 1);

    pad.write("s1", "b1", "k1", "plain text", now()).unwrap();
    let read = pad.read("s1", "b1", Some("k1"), now()).unwrap();
    assert_eq!(read["content"], "plain text");
}

#[test]
fn read_all_lists_every_pair_and_bytes() {
    let pad = Scratchpad::new();
    pad.write("s", "b", "a", "one", now()).unwrap();
    pad.write("s", "b", "b", "{\"x\":2}", now()).unwrap();
    let view = pad.read("s", "b", None, now()).unwrap();
    assert_eq!(view["entries"].as_array().unwrap().len(), 2);
    assert_eq!(view["total_bytes"].as_u64(), Some(10));
}

#[test]
fn missing_key_and_missing_session() {
    let pad = Scratchpad::new();
    let err = pad.read("s", "b", Some("nope"), now()).unwrap_err();
    assert_eq!(err.code, ErrorCode::ItemNotFound);
    let view = pad.read("ghost", "b", None, now()).unwrap();
    assert_eq!(view["entries"].as_array().unwrap().len(), 0);
    assert_eq!(
        pad.clear("ghost", "b", Some("x"), now()).map_err(|_| ()),
        Ok(())
    );
}

#[test]
fn byte_quota_rejected_and_state_preserved() {
    let pad = Scratchpad::new();
    pad.write("s", "b", "k", "x", now()).unwrap();
    let blob = "y".repeat(MAX_TOTAL_BYTES + 1);
    let err = pad.write("s", "b", "blob", &blob, now()).unwrap_err();
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    assert!(err.message.contains("QUOTA_EXCEEDED"));
    // The failed write left the scratchpad untouched.
    let view = pad.read("s", "b", None, now()).unwrap();
    assert_eq!(view["entries"].as_array().unwrap().len(), 1);
    assert_eq!(view["total_bytes"].as_u64(), Some(1));
    // Overwriting past the cap is also rejected.
    let err = pad.write("s", "b", "k", &blob, now()).unwrap_err();
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    // An exact-cap overwrite is allowed.
    let blob = "y".repeat(MAX_TOTAL_BYTES);
    pad.write("s", "b", "k", &blob, now()).unwrap();
}

#[test]
fn key_cap_rejected() {
    let pad = Scratchpad::new();
    for i in 0..MAX_KEYS {
        pad.write("s", "b", &format!("k{i}"), "v", now()).unwrap();
    }
    let err = pad.write("s", "b", "overflow", "v", now()).unwrap_err();
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    // Overwrites within the cap still work.
    pad.write("s", "b", "k0", "v2", now()).unwrap();
}

#[test]
fn session_and_bank_isolation() {
    let pad = Scratchpad::new();
    pad.write("s1", "b1", "k", "secret-a", now()).unwrap();
    pad.write("s2", "b1", "k", "secret-b", now()).unwrap();
    pad.write("s1", "b2", "k", "secret-c", now()).unwrap();
    let a = pad.read("s1", "b1", Some("k"), now()).unwrap();
    let b = pad.read("s2", "b1", Some("k"), now()).unwrap();
    let c = pad.read("s1", "b2", Some("k"), now()).unwrap();
    assert_eq!(a["content"], "secret-a");
    assert_eq!(b["content"], "secret-b");
    assert_eq!(c["content"], "secret-c");
}

#[test]
fn clear_removes_key_or_whole_session() {
    let pad = Scratchpad::new();
    pad.write("s", "b", "a", "1", now()).unwrap();
    pad.write("s", "b", "b", "2", now()).unwrap();
    pad.clear("s", "b", Some("a"), now()).unwrap();
    let view = pad.read("s", "b", None, now()).unwrap();
    assert_eq!(view["entries"].as_array().unwrap().len(), 1);
    assert_eq!(view["total_bytes"].as_u64(), Some(1));
    pad.clear("s", "b", None, now()).unwrap();
    let view = pad.read("s", "b", None, now()).unwrap();
    assert_eq!(view["total_bytes"].as_u64(), Some(0));
    // Removing an already-missing key is idempotent.
    pad.clear("s", "b", Some("a"), now()).unwrap();
}

#[test]
fn ttl_evicts_inactive_sessions_only() {
    let pad = Scratchpad::with_ttl(Duration::from_secs(60));
    // Every scratchpad call takes `now` and drives the TTL from it, so this test
    // owns the clock outright. It used to sleep 30ms and then 40ms against a
    // 60ms TTL, which left 20ms of margin for the session that has to survive:
    // a loaded runner could overshoot the second sleep by 20ms and evict it,
    // failing a test whose code was unchanged. Advancing an injected instant
    // states the same timeline exactly and cannot drift.
    let base = Instant::now();
    let at = |offset: Duration| base + offset;

    pad.write("stale", "b", "k", "v", at(Duration::ZERO))
        .unwrap();
    // The fresh session is touched halfway through the TTL window.
    pad.write("fresh", "b", "k", "v", at(Duration::from_secs(30)))
        .unwrap();

    // At t=70s the stale session has been idle for the full 70s and is gone;
    // the fresh one has been idle for 40s, inside the 60s TTL, and survives.
    let at_check = at(Duration::from_secs(70));
    let err = pad.read("stale", "b", Some("k"), at_check).unwrap_err();
    assert_eq!(err.code, ErrorCode::ItemNotFound);
    let view = pad.read("fresh", "b", Some("k"), at_check).unwrap();
    assert_eq!(view["content"], "v");
}

#[test]
fn empty_session_or_bank_rejected() {
    let pad = Scratchpad::new();
    let err = pad.write("", "b", "k", "v", now()).unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
    let err = pad.write("s", "", "k", "v", now()).unwrap_err();
    assert_eq!(err.code, ErrorCode::MissingBank);
}
