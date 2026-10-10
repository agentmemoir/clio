// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the bounded head sink: what it keeps, what it drops, and
//! its flush.
//!
//! ## Owns
//! - The assertions for what the sink retains past its bound and for the write
//!   length the writer contract requires
//!
//! ## Does not own
//! - The sink itself (see `head_sink`)
//! - How a dump or a verifier consumes the retained head
//!
//! ## Boundary
//!
//! Test-only, and never compiled into production builds.

use super::*;

#[test]
fn the_head_sink_keeps_its_first_bytes_and_drops_the_rest() {
    let mut sink = HeadSink::new(4);
    sink.write_all(b"abcdef").expect("write past the bound");
    assert_eq!(sink.head(), b"abcd");
    // A later write that fits in the head is still kept.
    let mut sink = HeadSink::new(8);
    sink.write_all(b"abc").expect("write");
    sink.write_all(b"defghijkl").expect("write past the bound");
    assert_eq!(sink.head(), b"abcdefgh");
    // The full byte count is consumed even when the head is full: the writer
    // contract is that write returns the input's length.
    assert_eq!(sink.write(b"xy").expect("write"), 2);
    sink.flush().expect("flush is a no-op that succeeds");
    assert_eq!(sink.head(), b"abcdefgh");
}
