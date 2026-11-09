// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the streamed response body a POST route can
//! hand the transport: the acknowledgement bytes written first, the waits
//! between frames, and the keep-alive that fills a wait with no frame.
//!
//! ## Owns
//! - The frame-source seam a route implements
//! - The streamed body: its acknowledgement head and its source
//! - The keep-alive interval and the SSE frame encoding both callers share
//! - Writing the head and then one frame per wait to the response socket
//!
//! ## Does not own
//! - The status line, the response headers, or the socket (see `http`)
//! - Request reading, gates, and connection accounting (see `http`)
//! - Frame content: the route decides what one frame says
//!
//! ## Boundary
//!
//! Changes to how a streamed body is framed or paced should normally require
//! changes here. Changes to a header, a gate, or a route should not.
//!
//! Keep this module focused on the streamed body between route and socket.

use std::io::Write;
use std::time::Duration;

/// How long one wait for a frame lasts before a keep-alive fills it.
pub(crate) const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// The comment bytes that hold an idle connection open between frames.
const KEEP_ALIVE_COMMENT: &[u8] = b": keep-alive\r\n\r\n";

/// One source of already-framed stream bytes.
pub(crate) trait FrameSource: Send {
    /// The next frame, or `None` when no frame arrived inside `timeout`.
    fn next(&mut self, timeout: Duration) -> Option<Vec<u8>>;
}

/// A streamed response body: the bytes written before the first wait, and the
/// source every later frame comes from.
pub(crate) struct EventStream {
    /// The acknowledgement frame, written before the first wait.
    pub(crate) head: Vec<u8>,
    /// The frames that follow the head.
    pub(crate) source: Box<dyn FrameSource>,
}

impl EventStream {
    /// Write the head, then one frame per wait, flushing after every write.
    ///
    /// A wait that yields no frame writes a keep-alive comment. A write error
    /// ends the stream quietly: the peer is gone and there is nothing left to
    /// say. A flush error is returned, because it reports a lost connection to
    /// the caller that owns the socket.
    pub(crate) fn pump(&mut self, out: &mut impl Write) -> std::io::Result<()> {
        if !self.head.is_empty() {
            out.write_all(&self.head)?;
        }
        out.flush()?;
        loop {
            let bytes = match self.source.next(KEEP_ALIVE) {
                Some(frame) => frame,
                None => KEEP_ALIVE_COMMENT.to_vec(),
            };
            if out.write_all(&bytes).is_err() {
                return Ok(());
            }
            out.flush()?;
        }
    }
}

/// One SSE frame carrying `json` as its `data` line, shared by both callers.
///
/// The unit tests are the only caller until a route answers with a stream.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn frame(json: &str) -> Vec<u8> {
    format!("event: message\ndata: {json}\n\n").into_bytes()
}

#[cfg(test)]
#[path = "event_stream_tests.rs"]
mod event_stream_tests;
