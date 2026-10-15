// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the COPY text reader: the
//! column a row yields, the end marker, the null marker, rows without the
//! column, the escapes one value carries, and the refusal a read error makes.
//!
//! ## Owns
//! - Every assertion about `copy_column` and `copy_unescape`
//! - The failing reader the read-error case drives
//!
//! ## Does not own
//! - The reader those tests exercise (`restore_authority_copy` owns it)
//! - Where the dump table bytes come from (`restore_authority` owns that)
//!
//! ## Boundary
//!
//! Changes to how COPY text is parsed should normally require changes here.
//! Changes to how an archive is classified should not.
//!
//! Keep this module focused on the COPY text reader's tests.

use super::super::ClassifyRefusal;
use super::{copy_column, copy_unescape};
use clio_types::ErrorCode;
use std::io::BufRead;

struct FailingReader;

impl std::io::Read for FailingReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("closed"))
    }
}

impl BufRead for FailingReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        Err(std::io::Error::other("closed"))
    }

    fn consume(&mut self, _amt: usize) {}
}

fn column(input: &[u8], index: usize) -> Vec<String> {
    copy_column(input, index).expect("the read succeeds")
}

#[test]
fn copy_column_reads_the_chosen_column_of_every_row() {
    assert_eq!(column(b"a\tb\nc\td\n", 0), ["a", "c"]);
    assert_eq!(column(b"a\tb\nc\td\n", 1), ["b", "d"]);
}

#[test]
fn copy_column_reads_a_last_row_without_a_newline() {
    assert_eq!(column(b"a\tb", 1), ["b"]);
}

#[test]
fn copy_column_stops_at_the_end_marker() {
    assert_eq!(column(b"a\n\\.\nb\n", 0), ["a"]);
}

#[test]
fn copy_column_skips_a_null_value() {
    assert_eq!(column(b"a\t\\N\n", 1), [] as [std::string::String; 0]);
}

#[test]
fn copy_column_skips_a_row_without_the_column() {
    assert_eq!(column(b"a\nb\tc\n", 1), ["c"]);
}

#[test]
fn copy_column_reports_a_read_failure() {
    let result = copy_column(FailingReader, 0).expect_err("the read fails");
    assert!(matches!(result, ClassifyRefusal::Io(_)), "got {result:?}");
    assert_eq!(result.code(), ErrorCode::Internal);
}

#[test]
fn copy_unescape_restores_the_control_escapes() {
    assert_eq!(
        copy_unescape(b"\\t\\n\\r\\b\\f\\v"),
        "\t\n\r\u{0008}\u{000c}\u{000b}"
    );
}

#[test]
fn copy_unescape_keeps_an_escaped_backslash() {
    assert_eq!(copy_unescape(b"a\\\\b"), "a\\b");
}

#[test]
fn copy_unescape_drops_an_unknown_escape() {
    assert_eq!(copy_unescape(b"\\x"), "x");
}

#[test]
fn copy_unescape_keeps_a_trailing_backslash() {
    assert_eq!(copy_unescape(b"a\\"), "a\\");
}
