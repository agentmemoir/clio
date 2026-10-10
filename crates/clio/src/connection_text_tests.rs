// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the one sentence the CLI prints about a store: whether the
//! connection is encrypted and whether it stays on this machine.
//!
//! The managed Postgres address has no password, so the address shapes here
//! cover both a passwordless local store and a hosted one that does have one.
//! The sentence must not change shape because of the password either way.

use super::*;

#[test]
fn database_encryption_locality_covers_the_three_address_shapes() {
    assert_eq!(
        database_encryption_locality("postgres://clio:secret@127.0.0.1:34310/clio?sslmode=prefer"),
        "encrypted: no, this machine only"
    );
    assert_eq!(
        database_encryption_locality(
            "postgres://clio:secret@db.example.com:5432/clio?sslmode=require"
        ),
        "encrypted, remote"
    );
    assert_eq!(
        database_encryption_locality("/home/you/.local/share/clio/clio.db"),
        "not encrypted, this machine only"
    );
}

#[test]
fn a_managed_address_reads_the_same_without_a_password() {
    // The managed address is `postgres://clio@127.0.0.1:34310/clio`. It has to
    // reach the same sentence the hosted address above does, so removing the
    // password did not change what the summary says about the store.
    assert_eq!(
        database_encryption_locality("postgres://clio@127.0.0.1:34310/clio?sslmode=prefer"),
        "encrypted: no, this machine only"
    );
}
