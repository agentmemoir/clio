// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the real-binary end-to-end evidence for the encrypted sync
//! wire: a relay served by a spawned `clio`, two nodes with separate stores and
//! separate content-key files that share one sync key file, a caller-chosen
//! attribute name written on node A, pushed encrypted, pulled on node B, and
//! then the relay's own SQLite journal read directly. It asserts the relay's
//! stored row carries nothing the caller chose
//! and nothing a candidate dictionary can confirm, and that node B applied the
//! record under its real attribute key.
//!
//! The in-process tests in `clio-sync` prove the same properties one layer down,
//! through the library entry points. This one proves the CLI entry point a
//! deployment actually uses still produces them, which is the layer a regression
//! in flag handling or key supply would hide.
//!
//! ## Owns
//! - The spawned-binary two-node encrypted push, pull, and journal inspection
//! - The guess-confirmation oracle check against the relay's stored row
//!
//! ## Does not own
//! - The keyed derivations and the packaging helpers (`clio-sync`)
//! - The in-process journal tests (`client_wire_blindness_tests`)
//! - The spawned relay and node plumbing (`sync_wire_support`)
//!
//! ## Boundary
//!
//! Tests here break when the CLI stops producing an opaque encrypted row or
//! stops applying it under the real identifier; they never change either.
//!
//! Keep this module focused on the spawned-binary encrypted sync path.

use rusqlite::Connection;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

#[path = "sync_wire_support/mod.rs"]
mod sync_wire_support;
use sync_wire_support::{
    BANK, NodeDir, Relay, SYNC_KEY, TOKEN, clio, expect_ok, json_stdout, relay_rows, run,
};

/// The caller-chosen attribute name this harness pushes. Synthetic: the
/// property under test is that it does not reach the relay, not what it says.
const ATTR_NAME: &str = "synthetic_attr_name";

/// Write one persona stable entry keyed by a caller-chosen attribute name, on
/// the node whose store is `db`.
fn write_attribute(node: &NodeDir, db: &Path) {
    let args = vec![
        "persona", "stable", ATTR_NAME, "terse", "--bank", BANK, "--output", "json",
    ];
    let written = run(node.path(), db, &args);
    expect_ok(&args, &written);
    let payload = json_stdout(&args, &written);
    assert_eq!(
        payload.get("ok").and_then(Value::as_bool),
        Some(true),
        "the attribute was admitted on the writing node: {payload}"
    );
}

/// The 32 raw key bytes both nodes read, derived from the synthetic harness
/// secret so the shared key file can never carry a real key.
fn shared_sync_key_bytes() -> [u8; 32] {
    let hex = clio_store::sha256_hex(SYNC_KEY.as_bytes(), "sync-wire-harness-key");
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("hex digest");
    }
    bytes
}

/// Write the shared key file both nodes read through `CLIO_SYNC_KEY_FILE`,
/// owner-only because the loader refuses a file another account can read.
fn write_shared_sync_key(dir: &NodeDir) -> PathBuf {
    let path = dir.path().join("shared-sync.key");
    let bytes = shared_sync_key_bytes();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("the shared sync key file can be created");
        file.write_all(&bytes).expect("the sync key can be written");
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, bytes).expect("the sync key can be written");
    }
    path
}

/// Run one sync verb against a node's store with the shared key file set
/// through `CLIO_SYNC_KEY_FILE`, the stored route the CLI now uses.
fn run_with_sync_key(node: &NodeDir, db: &Path, key_file: &Path, args: &[&str]) -> Output {
    clio(node.path())
        .env("CLIO_SYNC_KEY_FILE", key_file)
        .args(args)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("spawn clio {}: {e}", args.join(" ")))
}

/// Push every pending mutation from one node to the relay.
fn push(node: &NodeDir, db: &Path, key_file: &Path, remote: &str) -> Value {
    let args = vec![
        "sync", "push", "--remote", remote, "--token", TOKEN, "--banks", BANK, "--output", "json",
    ];
    let pushed = run_with_sync_key(node, db, key_file, &args);
    expect_ok(&args, &pushed);
    json_stdout(&args, &pushed)
}

/// Pull from the relay into one node's store.
fn pull(node: &NodeDir, db: &Path, key_file: &Path, remote: &str) {
    let args = vec![
        "sync", "pull", "--remote", remote, "--token", TOKEN, "--banks", BANK, "--output", "json",
    ];
    let pulled = run_with_sync_key(node, db, key_file, &args);
    expect_ok(&args, &pulled);
}

/// The relay's stored row leaks nothing the caller chose and nothing a candidate
/// dictionary can confirm: one persona attribute name written on node A, pushed
/// encrypted through the real binary, and read back off the relay's own table.
#[test]
fn a_real_binary_encrypted_push_leaves_the_relay_nothing_to_read() {
    let node_a = NodeDir::new("node-a");
    let node_b = NodeDir::new("node-b");
    let relay_dir = NodeDir::new("relay");
    let key_dir = NodeDir::new("shared-key");
    let sync_key = write_shared_sync_key(&key_dir);
    let relay_db = relay_dir.db("relay.sqlite");
    let db_a = node_a.db("a.sqlite");
    let db_b = node_b.db("b.sqlite");

    write_attribute(&node_a, &db_a);
    let relay = Relay::start(&relay_dir, &relay_db);
    let remote = relay.remote().to_owned();

    let pushed = push(&node_a, &db_a, &sync_key, &remote);
    assert_eq!(
        pushed.get("encryption").and_then(Value::as_bool),
        Some(true),
        "the push sealed its content: {pushed}"
    );

    // Nothing the caller chose reaches the relay, in any stored column.
    let rows = relay_rows(&relay_db);
    assert!(
        !rows.is_empty(),
        "the relay stored no row at all for bank {BANK}"
    );
    let persona: Vec<_> = rows.iter().filter(|r| r.entity_kind == "persona").collect();
    assert_eq!(persona.len(), 1, "the persona row reaches the relay");
    for row in &rows {
        let bytes = row.stored_bytes();
        assert_eq!(row.encrypted, 1, "the row is sealed: {bytes}");
        assert!(
            !bytes.contains(ATTR_NAME),
            "the attribute name must not reach the relay: {bytes}"
        );
    }
    let sealed = persona[0];
    assert!(
        sealed.is_handle_shaped(),
        "the identifier column is the opaque handle, not the attribute name: {}",
        sealed.entity_id
    );

    // The stored idempotency key is not the published unkeyed hash of a guess.
    // The relay already holds the stamp that preimage needs, so one comparison
    // would confirm the attribute name if the key were that hash.
    let oracle = clio_store::sha256_hex(
        b"am-sync-mut",
        &format!("{ATTR_NAME}|{}|{}", sealed.transaction_time, sealed.op),
    );
    assert_ne!(
        sealed.sync_mutation_id, oracle,
        "a guessed attribute name must not confirm against the stored key"
    );
    assert_ne!(
        sealed.content_hash,
        clio_store::sha256_hex(b"am-sync-hash", ATTR_NAME),
        "and the stored digest is not the hash of the guessed value either"
    );

    // Node B pulls and applies it under the real attribute key, so the handle is
    // resolved out of the sealed payload rather than standing in for the
    // identifier in the store's business-identifier column.
    pull(&node_b, &db_b, &sync_key, &remote);
    let peer = Connection::open(&db_b).expect("open the pulling node's database");
    let attr_key: String = peer
        .query_row(
            "select attr_key from persona_stable_entries where bank_id = ?1",
            [BANK],
            |row| row.get(0),
        )
        .expect("the pulled node applied the record under its real attribute key");
    assert_eq!(
        attr_key, ATTR_NAME,
        "the applied row is keyed by the caller's attribute name"
    );
    let keyed_by_handle: i64 = peer
        .query_row(
            "select count(*) from persona_stable_entries where attr_key = ?1",
            [sealed.entity_id.as_str()],
            |row| row.get(0),
        )
        .expect("count rows keyed by the handle");
    assert_eq!(
        keyed_by_handle, 0,
        "no applied row is keyed by the opaque handle in place of an identifier"
    );
}

/// A second push of the same record adds no second relay row, so the keyed
/// idempotency key still makes a redelivery a duplicate on the real binary path.
#[test]
fn a_second_push_of_the_same_record_adds_no_second_relay_row() {
    let node_a = NodeDir::new("dup-node-a");
    let relay_dir = NodeDir::new("dup-relay");
    let key_dir = NodeDir::new("dup-shared-key");
    let sync_key = write_shared_sync_key(&key_dir);
    let relay_db = relay_dir.db("relay.sqlite");
    let db_a = node_a.db("a.sqlite");

    write_attribute(&node_a, &db_a);
    let relay = Relay::start(&relay_dir, &relay_db);
    let remote = relay.remote().to_owned();

    for attempt in 1..=2 {
        push(&node_a, &db_a, &sync_key, &remote);
        let rows = relay_rows(&relay_db);
        assert_eq!(
            rows.len(),
            1,
            "push {attempt} left {} rows on the relay, so a redelivery was not a duplicate",
            rows.len()
        );
    }
}
