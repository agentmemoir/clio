// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the download's second half: the
//! streaming write that fills `<dest>.part`, the byte-count check, and the rename
//! into place, driven with an in-memory reader rather than a socket.
//!
//! ## Owns
//! - `stream_to_file` and `part_path` behaviour under test for `setup_extract_model`
//!
//! ## Does not own
//! - Whether a run needs the file, and where the resolved stack says it goes (see
//!   `setup_extract_model_tests`)
//! - The real fetcher's transport over a socket (see
//!   `setup_extract_model_http_tests`)
//!
//! ## Boundary
//!
//! Changes to how bytes reach the destination, or to what a failed write leaves
//! behind, should normally update tests here. Changes to when the fetch runs should
//! not.

use super::*;
use std::io::Cursor;

/// A body that hands over `remaining` bytes and then fails, the way a dropped
/// connection does.
struct BreakingReader {
    remaining: usize,
}

impl Read for BreakingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::other("the connection dropped"));
        }
        let count = buf.len().min(self.remaining);
        buf[..count].fill(b'z');
        self.remaining -= count;
        Ok(count)
    }
}

#[test]
fn a_download_that_breaks_leaves_neither_a_part_file_nor_a_model() {
    let dir = temp_dir("stream-breaks");
    let dest = dir.join(GGUF_FILE_NAME);
    let mut progress = Vec::new();

    let err = stream_to_file(
        &mut BreakingReader { remaining: 1_000 },
        &dest,
        Some(10_000),
        &mut |written| progress.push(written),
    )
    .expect_err("a body that stops early must fail the download");

    assert!(err.contains("the download stopped"), "{err}");
    assert_eq!(progress, vec![1_000], "progress is reported per chunk");
    assert!(
        !part_path(&dest).exists(),
        "the partial file is removed on the error path"
    );
    assert!(!dest.exists(), "a failed download never becomes the model");
}

#[test]
fn a_download_shorter_than_advertised_is_refused_and_leaves_no_part_file() {
    let dir = temp_dir("stream-short");
    let dest = dir.join(GGUF_FILE_NAME);

    let err = stream_to_file(
        &mut Cursor::new(vec![b'z'; 500]),
        &dest,
        Some(1_000),
        &mut |_| {},
    )
    .expect_err("a short body must fail the download");

    assert!(err.contains("500 of 1000 bytes"), "{err}");
    assert!(
        !part_path(&dest).exists(),
        "the partial file is removed when the length does not match"
    );
    assert!(!dest.exists(), "a short download never becomes the model");
}

#[test]
fn a_complete_download_is_renamed_into_place() {
    let dir = temp_dir("stream-complete");
    let dest = dir.join(GGUF_FILE_NAME);
    let mut progress = Vec::new();
    let body = vec![b'z'; 200_000];

    let written = stream_to_file(&mut Cursor::new(body), &dest, Some(200_000), &mut |bytes| {
        progress.push(bytes);
    })
    .expect("a matching body is written");

    assert_eq!(written, 200_000);
    assert_eq!(
        progress,
        vec![65_536, 131_072, 196_608, 200_000],
        "one report per 64 KiB chunk, ending on the last byte"
    );
    assert!(
        !part_path(&dest).exists(),
        "the partial file was renamed away"
    );
    assert_eq!(
        std::fs::metadata(&dest)
            .expect("the model is on disk")
            .len(),
        200_000
    );
}
