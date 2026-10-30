// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pull loop's requested-bank guard: a relay that
//! answers one bank's pull with another bank's row is refused on that row's
//! bank stamp alone, before serialization and decryption, with no journal
//! write, no dead letter, no store mutation, and no cursor advance.
//!
//! ## Owns
//! - Refusal tests for foreign-bank plain, valid-cipher, and invalid-cipher
//!   rows, for a mixed page, and the same-bank control
//! - The one-page hostile relay used by these tests only
//!
//! ## Does not own
//! - The guard itself (`client`)
//! - Wire-level hostile-relay framing tests (`client_wire_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the requested-bank guard changes; they never change
//! it.
//!
//! Keep this module focused on requested-bank pull tests.

use crate::client::client_tests::{cfg, create_item, item, paired_key, sqlite};
use crate::client::pull;
use crate::crypto::{SyncCipher, SyncCipherKey};
use crate::envelope_fixtures::{seal_content_for, seal_own_content};
use crate::protocol::{SyncContent, SyncMutation, encode_cursor};
use clio_store::{DeadLetterStatus, SqliteStore, Store, SyncStore};
use clio_types::AmError;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

const BANK_A: &str = "bA";
const BANK_B: &str = "bB";
const VICTIM: &str = "victim-b";
const VICTIM_GIST: &str = "keep me";

/// One hostile relay that answers the first `times` connections with
/// `mutations` as the only pull page, then stops. It ignores the requested
/// bank: that mismatch is what the pull loop must catch.
fn hostile_page(mutations: &[SyncMutation], times: usize) -> String {
    let page = serde_json::to_string(&json!({
        "mutations": mutations,
        "next_cursor": encode_cursor(BANK_A, 1),
        "has_more": false,
    }))
    .expect("page json");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        for stream in listener.incoming().take(times) {
            let mut stream = stream.expect("accept");
            read_declared_request(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            stream.write_all(response.as_bytes()).expect("respond");
        }
    });
    addr
}

/// Read one client request to its declared end: the head terminator plus the
/// `Content-Length` body, so the page is written after the client finished
/// sending and the close never resets the client mid-read.
fn read_declared_request(stream: &mut std::net::TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).expect("read request");
        buf.extend_from_slice(&chunk[..n]);
        let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let declared = String::from_utf8_lossy(&buf[..pos])
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        if buf.len() >= pos + 4 + declared {
            return;
        }
    }
}

/// One plain `item` discard stamped with `bank`: the row a hostile relay
/// serves to reach into a bank the pull never asked for.
fn foreign_discard(bank: &str, entity_id: &str) -> SyncMutation {
    SyncMutation {
        sync_mutation_id: format!("m-fb-{bank}-{entity_id}"),
        bank: bank.to_owned(),
        entity_kind: "item".into(),
        entity_id: entity_id.to_owned(),
        op: "discard".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "hostile".into(),
        content: SyncContent::Plain(json!({})),
        content_hash: "h".into(),
        admission_receipt: None,
    }
}

/// One valid `item` upsert stamped with `bank`. Callers seal it: a client
/// holding a cipher key refuses plaintext rows at decrypt time.
fn upsert_row(bank: &str, id: &str) -> SyncMutation {
    let document = serde_json::to_value(item(bank, id, "s1", "2026-01-01T00:00:00Z", "from relay"))
        .expect("item json");
    SyncMutation {
        sync_mutation_id: format!("m-up-{bank}-{id}"),
        bank: bank.to_owned(),
        entity_kind: "item".into(),
        entity_id: id.to_owned(),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "hostile".into(),
        content: SyncContent::Plain(document),
        content_hash: "h".into(),
        admission_receipt: None,
    }
}

fn cipher_row(key: &SyncCipherKey) -> SyncMutation {
    let mut row = foreign_discard(BANK_B, VICTIM);
    row.content = SyncContent::Cipher(SyncCipher {
        alg: "xchacha20poly1305".into(),
        key_id: Some(key.key_id()),
        nonce_b64: "AAAA".into(),
        ciphertext_b64: "AAAA".into(),
    });
    row
}

fn assert_bank_error(err: &AmError) {
    assert!(
        err.message.contains(BANK_A) && err.message.contains(BANK_B),
        "the refusal must name both banks: {}",
        err.message
    );
}

/// Bank B survived the pull with no journal write, no dead letter, and no
/// cursor advance: everything a wrong-bank row must leave untouched.
fn assert_victim_untouched(b: &Arc<SqliteStore>) {
    let victim = b
        .get_memory_item(BANK_B, VICTIM)
        .expect("victim read")
        .expect("victim still stored");
    assert!(victim.tx_until.is_none(), "victim-b must still be active");
    assert_eq!(victim.gist.as_deref(), Some(VICTIM_GIST));
    assert!(
        b.journal_list_since(BANK_B, 0, 200)
            .expect("journal")
            .is_empty(),
        "no journal write may land in bank B"
    );
    for bank in [BANK_A, BANK_B] {
        assert_eq!(
            b.dead_letter_count(bank, DeadLetterStatus::Open)
                .expect("dead letters"),
            0,
            "no dead letter may be written for {bank}"
        );
    }
    let cursor = b.cursor_get(BANK_A, "pull").expect("cursor");
    assert!(
        cursor.as_deref().unwrap_or_default().is_empty(),
        "no cursor advance: {cursor:?}"
    );
}

#[test]
fn foreign_bank_plain_row_is_refused_with_bank_b_untouched() {
    let b = sqlite();
    create_item(&b, BANK_B, VICTIM, VICTIM_GIST);
    let addr = hostile_page(&[foreign_discard(BANK_B, VICTIM)], 1);
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK_A.to_owned()]).expect_err("a foreign-bank row is refused");
    assert_bank_error(&err);
    assert_victim_untouched(&b);
}

#[test]
fn foreign_bank_cipher_row_is_refused_before_it_opens() {
    let b = sqlite();
    create_item(&b, BANK_B, VICTIM, VICTIM_GIST);
    // Sealed under the key this node holds, so the only thing standing between
    // the row and apply is the bank stamp.
    let sealed = seal_content_for(foreign_discard(BANK_B, VICTIM), &paired_key(), &json!({}));
    let addr = hostile_page(&[sealed], 1);
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK_A.to_owned()]).expect_err("a foreign-bank cipher row is refused");
    assert_bank_error(&err);
    assert_victim_untouched(&b);
}

#[test]
fn foreign_bank_invalid_cipher_is_refused_on_the_bank_not_the_open() {
    let b = sqlite();
    create_item(&b, BANK_B, VICTIM, VICTIM_GIST);
    // Unopenable ciphertext: if the row reached the open, the pull would
    // quarantine it and return `Ok`, never this error.
    let addr = hostile_page(&[cipher_row(&paired_key())], 1);
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK_A.to_owned()]).expect_err("the bank check fires first");
    assert_bank_error(&err);
    assert_victim_untouched(&b);
}

#[test]
fn mixed_page_errors_at_the_foreign_row_and_leaves_bank_b_untouched() {
    let b = sqlite();
    create_item(&b, BANK_B, VICTIM, VICTIM_GIST);
    let addr = hostile_page(
        &[
            seal_own_content(upsert_row(BANK_A, "i-a")),
            seal_content_for(foreign_discard(BANK_B, VICTIM), &paired_key(), &json!({})),
        ],
        1,
    );
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK_A.to_owned()]).expect_err("the page errors at the B row");
    assert_bank_error(&err);
    assert_victim_untouched(&b);
    // The error fired at the B row, after the same-bank row applied.
    assert!(
        b.get_memory_item(BANK_A, "i-a")
            .expect("applied row read")
            .is_some()
    );
}

#[test]
fn same_bank_rows_still_apply_and_replay_deduplicates() {
    let b = sqlite();
    let addr = hostile_page(&[seal_own_content(upsert_row(BANK_A, "i-ctl"))], 2);
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    pull(&cfg_b, &[BANK_A.to_owned()]).expect("same-bank row applies");
    assert!(
        b.get_memory_item(BANK_A, "i-ctl")
            .expect("row read")
            .is_some()
    );
    let cursor = b
        .cursor_get(BANK_A, "pull")
        .expect("cursor")
        .unwrap_or_default();
    assert!(!cursor.is_empty(), "the cursor advanced: {cursor:?}");
    pull(&cfg_b, &[BANK_A.to_owned()]).expect("replay pull");
    let rows = b.journal_list_since(BANK_A, 0, 200).expect("journal");
    assert_eq!(rows.len(), 1, "the replay deduplicates: {rows:?}");
}
