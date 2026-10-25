// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the transfer's second half: the
//! streaming write into this run's temporary file, the length and digest checks,
//! the rename into place, and the local-copy install, driven with an in-memory
//! reader rather than a socket.
//!
//! ## Owns
//! - `stream_to_file`, `part_path`, and `install_local` behaviour under test
//!
//! ## Does not own
//! - Whether a run needs the file, and where the resolved stack says it goes (see
//!   `setup_extract_model_tests`)
//! - The real fetcher's transport over a socket (see
//!   `setup_extract_model_http_tests`)
//!
//! ## Boundary
//!
//! Changes to how bytes reach the destination, or to what a refused transfer
//! leaves behind, should normally update tests here. Changes to when the fetch
//! runs should not.

use super::*;
use crate::compose_assets::GGUF_FILE_NAME;
use crate::setup_support_tests::temp_dir;
use std::io::Cursor;

/// The lowercase-hex SHA-256 of `bytes`, the form a caller passes in.
fn digest_of(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

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
        &digest_of(b""),
        &mut |written| progress.push(written),
    )
    .expect_err("a body that stops early must fail the download");

    assert!(err.contains("the download stopped"), "{err}");
    assert_eq!(progress, vec![1_000], "progress is reported per chunk");
    assert!(
        !part_path(&dest).exists(),
        "the temporary file is removed on the error path"
    );
    assert!(!dest.exists(), "a failed download never becomes the model");
}

#[test]
fn a_download_shorter_than_advertised_is_refused_and_leaves_no_part_file() {
    let dir = temp_dir("stream-short");
    let dest = dir.join(GGUF_FILE_NAME);
    let body = vec![b'z'; 500];

    let err = stream_to_file(
        &mut Cursor::new(body.clone()),
        &dest,
        Some(1_000),
        &digest_of(&body),
        &mut |_| {},
    )
    .expect_err("a short body must fail the download");

    assert!(err.contains("500 of 1000 bytes"), "{err}");
    assert!(
        !part_path(&dest).exists(),
        "the temporary file is removed when the length does not match"
    );
    assert!(!dest.exists(), "a short download never becomes the model");
}

/// An answer that states no length has nothing to compare the body with, so it is
/// refused rather than installed.
#[test]
fn a_body_with_no_stated_length_is_refused_and_leaves_no_file() {
    let dir = temp_dir("stream-no-length");
    let dest = dir.join(GGUF_FILE_NAME);
    let body = vec![b'z'; 500];

    let err = stream_to_file(
        &mut Cursor::new(body.clone()),
        &dest,
        None,
        &digest_of(&body),
        &mut |_| {},
    )
    .expect_err("an answer with no length must be refused");

    assert_eq!(
        err,
        "the answer stated no length, so the download was refused; run `clio setup` again"
    );
    assert!(!dest.exists(), "nothing is installed without a length");
    assert!(
        !part_path(&dest).exists(),
        "no temporary file is created without a length"
    );
}

/// A `200` that carries no bytes is the empty-body defect: it must fail rather
/// than become a zero-byte model the container cannot load.
#[test]
fn a_zero_byte_body_is_refused_and_leaves_no_file() {
    let dir = temp_dir("stream-empty");
    let dest = dir.join(GGUF_FILE_NAME);

    let err = stream_to_file(
        &mut Cursor::new(Vec::new()),
        &dest,
        Some(0),
        &digest_of(b""),
        &mut |_| {},
    )
    .expect_err("an empty body must be refused");

    assert_eq!(
        err,
        "the answer carried no bytes, so the download was refused; run `clio setup` again"
    );
    assert!(!dest.exists(), "an empty body never becomes the model");
    assert!(
        !part_path(&dest).exists(),
        "no temporary file survives an empty body"
    );
}

#[test]
fn a_complete_download_is_renamed_into_place() {
    let dir = temp_dir("stream-complete");
    let dest = dir.join(GGUF_FILE_NAME);
    let mut progress = Vec::new();
    let body = vec![b'z'; 200_000];

    let written = stream_to_file(
        &mut Cursor::new(body.clone()),
        &dest,
        Some(200_000),
        &digest_of(&body),
        &mut |bytes| {
            progress.push(bytes);
        },
    )
    .expect("a matching body is written");

    assert_eq!(written, 200_000);
    assert_eq!(
        progress,
        vec![65_536, 131_072, 196_608, 200_000],
        "one report per 64 KiB chunk, ending on the last byte"
    );
    assert!(
        !part_path(&dest).exists(),
        "the temporary file was renamed away"
    );
    assert_eq!(
        std::fs::metadata(&dest)
            .expect("the model is on disk")
            .len(),
        200_000
    );
}

/// A digest that is not the published one is refused before the rename, so bytes
/// from anywhere in the redirect chain can never become the model.
#[test]
fn a_digest_that_does_not_match_is_refused_and_removes_both_files() {
    let dir = temp_dir("stream-digest");
    let dest = dir.join(GGUF_FILE_NAME);
    std::fs::write(&dest, b"an older model").expect("write the older model");
    let body = vec![b'z'; 200_000];
    let published = digest_of(&body);

    let err = stream_to_file(
        &mut Cursor::new(body),
        &dest,
        Some(200_000),
        &"0".repeat(64),
        &mut |_| {},
    )
    .expect_err("a body whose digest differs must be refused");

    assert!(
        err.contains("published checksum"),
        "the refusal names the check: {err}"
    );
    assert!(
        err.contains(&"0".repeat(64)),
        "it names what it expected: {err}"
    );
    assert!(err.contains(&published), "it names what it found: {err}");
    assert!(
        !dest.exists(),
        "the unchecked file at the destination goes too"
    );
    assert!(
        !part_path(&dest).exists(),
        "the temporary file is removed on a digest mismatch"
    );
}

/// The temporary file belongs to one run: its name is not the shared
/// `<dest>.part`, and a `<dest>.part` another run is writing is left alone.
#[test]
fn the_temporary_file_is_this_run_s_own_and_another_run_s_is_left_alone() {
    let dir = temp_dir("stream-unique");
    let dest = dir.join(GGUF_FILE_NAME);
    let shared = dest.with_file_name(format!("{GGUF_FILE_NAME}.part"));
    std::fs::write(&shared, b"another run is writing this").expect("write the other run's part");

    let part = part_path(&dest);
    assert_ne!(part, shared, "the temporary file is not the shared name");
    assert!(
        part.to_string_lossy()
            .contains(&format!(".{}.part", std::process::id())),
        "the name carries the process that owns it: {}",
        part.display()
    );

    let body = vec![b'z'; 200_000];
    stream_to_file(
        &mut Cursor::new(body.clone()),
        &dest,
        Some(200_000),
        &digest_of(&body),
        &mut |_| {},
    )
    .expect("a matching body is written");

    assert_eq!(
        std::fs::read(&shared).expect("the other run's part is still there"),
        b"another run is writing this",
        "this run must never delete or corrupt another run's file"
    );
    assert!(
        !part.exists(),
        "this run's own temporary file was renamed away"
    );
}

/// A local copy takes the same checks as a download, without a socket.
#[test]
fn a_local_copy_is_installed_with_the_checksum_it_was_given() {
    let dir = temp_dir("stream-local");
    let source = dir.join("copy.gguf");
    let dest = dir.join(GGUF_FILE_NAME);
    let body = vec![b'z'; 100_000];
    std::fs::write(&source, &body).expect("write the local copy");

    let written = install_local(&source, &dest, &digest_of(&body), &mut |_| {})
        .expect("a local copy that matches is installed");

    assert_eq!(written, 100_000);
    assert_eq!(std::fs::read(&dest).expect("the model is on disk"), body);
    assert!(
        !part_path(&dest).exists(),
        "the temporary file was renamed away"
    );
}
