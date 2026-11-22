// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! This module is responsible for testing the per-frame byte cap on the stdio
//! transport: a frame over the cap is refused and a frame at the cap is served.
//!
//! ## Owns
//! - The recorded frame cap and the refusal message for an oversized frame
//! - The accept boundary at exactly the cap
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
fn frame_of_exactly_the_cap_is_accepted() {
    let mut input = vec![b' '; MAX_FRAME_BYTES - 1];
    input.push(b'\n');
    let output = serve_unanswered(input).expect("a frame at the cap is served");
    assert!(
        output.is_empty(),
        "a blank frame writes nothing: {output:?}"
    );
}
