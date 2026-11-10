// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit tests of the shared SSE frame
//! encoding.
//!
//! ## Owns
//! - The exact bytes one frame carries, including its line endings
//!
//! ## Does not own
//! - The frame source, the streamed body, or its write loop (see
//!   `event_stream`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;

/// The frame of a small JSON value, byte for byte.
#[test]
fn a_frame_is_one_event_and_one_data_line() {
    assert_eq!(
        frame("{\"ok\":true}"),
        b"event: message\ndata: {\"ok\":true}\n\n".to_vec()
    );
}

/// A JSON value with no newline stays on its one `data` line.
#[test]
fn a_frame_keeps_a_single_line_value_on_one_data_line() {
    let text = String::from_utf8(frame(r#"{"note":"a b"}"#)).expect("a frame is UTF-8");
    assert_eq!(
        text, "event: message\ndata: {\"note\":\"a b\"}\n\n",
        "{text:?}"
    );
    assert_eq!(text.matches("data:").count(), 1, "{text:?}");
}
