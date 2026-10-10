// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use crate::chunk::{Turn, partition_session};

#[test]
fn empty_session_yields_no_chunks() {
    let chunks = partition_session(&[], 2);
    assert!(chunks.is_empty(), "{chunks:?}");
}

#[test]
fn partitions_by_max_turns() {
    let turns = vec![Turn::text("a"), Turn::text("b"), Turn::text("c")];
    let chunks = partition_session(&turns, 2);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chunk_id, "c0");
    assert_eq!(chunks[0].source_text, "a\nb");
    assert_eq!(chunks[0].turn_indices, vec![0, 1]);
    assert_eq!(chunks[1].chunk_id, "c2");
    assert_eq!(chunks[1].source_text, "c");
    assert_eq!(chunks[1].turn_indices, vec![2]);
}

#[test]
fn zero_cap_treated_as_one() {
    let turns = vec![Turn::text("x"), Turn::text("y")];
    let chunks = partition_session(&turns, 0);
    assert_eq!(chunks.len(), 2);
}
