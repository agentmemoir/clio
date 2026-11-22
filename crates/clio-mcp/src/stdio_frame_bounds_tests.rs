// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the per-frame byte cap on the stdio
//! transport: a frame at the cap is delivered intact and a frame one byte past
//! it is refused.
//!
//! ## Owns
//! - The recorded frame cap and the refusal message for an oversized frame
//! - The accept boundary at exactly the cap, proving the frame is not truncated
//! - The refusal boundary when the newline lands at the cap plus one
//!
//! ## Does not own
//! - Framing, trimming, and the serving loop (see `stdio`)
//! - The dispatcher's request rules (see `stateless`)
//!
//! ## Boundary
//!
//! Changes to the frame cap or its refusal should normally require changes here.
//! Changes to framing behavior or the request rules should not require changes here.
//!
//! Keep this module focused on the per-frame byte cap.

use super::{MAX_FRAME_BYTES, serve_frames};
use std::io::{BufReader, Cursor};

fn serve_unanswered(input: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    serve_frames(BufReader::new(Cursor::new(input)), &mut output, |_| None)?;
    Ok(output)
}

#[test]
fn recorded_frame_cap_is_pinned() {
    assert_eq!(
        MAX_FRAME_BYTES,
        10 * 1024 * 1024,
        "a raised frame cap is a defect, not a tuning choice"
    );
}

#[test]
fn oversized_frame_is_refused_with_the_cap_message() {
    let error = serve_unanswered(vec![b' '; MAX_FRAME_BYTES + 1])
        .expect_err("a frame over the cap is refused");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "frame exceeds the 10 MiB frame cap");
}

#[test]
fn frame_of_exactly_the_cap_is_delivered_intact() {
    let mut input = vec![b'x'; MAX_FRAME_BYTES - 1];
    input.push(b'\n');
    let mut delivered = None;
    let mut output = Vec::new();
    serve_frames(BufReader::new(Cursor::new(input)), &mut output, |frame| {
        delivered = Some(frame.len());
        None
    })
    .expect("a frame at the cap is delivered");
    assert_eq!(
        delivered,
        Some(MAX_FRAME_BYTES - 1),
        "the frame is not truncated"
    );
    assert!(
        output.is_empty(),
        "an unanswered frame writes nothing: {output:?}"
    );
}

#[test]
fn frame_with_newline_at_cap_plus_one_is_refused() {
    let mut input = vec![b'x'; MAX_FRAME_BYTES];
    input.push(b'\n');
    let error = serve_unanswered(input).expect_err("the newline sits one byte past the cap");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "frame exceeds the 10 MiB frame cap");
}
