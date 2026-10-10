// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bounded head sink: a `Write` target that
//! keeps only its first bytes and drops the rest.
//!
//! ## Owns
//! - `HeadSink`, the bounded-memory head reader the archive verify pass uses
//!
//! ## Does not own
//! - What the head is compared with (see `verify`)
//! - The container reader (see `container`)
//!
//! ## Boundary
//!
//! Changes to which bytes are kept should normally require changes here.
//! Changes to verification rules should not.
//!
//! Keep this module focused on the bounded head sink.

use std::io::Write;

/// A `Write` sink that keeps only its first `keep` bytes and drops the rest.
///
/// The key member is never buffered whole and never written to disk, so the
/// head check costs sixteen bytes of memory.
pub(crate) struct HeadSink {
    keep: usize,
    head: Vec<u8>,
}

impl HeadSink {
    pub(crate) fn new(keep: usize) -> Self {
        Self {
            keep,
            head: Vec::with_capacity(keep),
        }
    }

    pub(crate) fn head(&self) -> &[u8] {
        &self.head
    }
}

impl Write for HeadSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.head.len() < self.keep {
            let want = (self.keep - self.head.len()).min(buf.len());
            self.head.extend_from_slice(&buf[..want]);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "head_sink_tests.rs"]
mod tests;
