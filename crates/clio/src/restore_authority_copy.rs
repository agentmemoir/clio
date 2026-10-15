// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading plaintext column values out of a
//! Postgres COPY text dump table.
//!
//! ## Owns
//! - `copy_column`, which yields one column's value from every row
//! - `copy_unescape`, which undoes the COPY text escapes
//!
//! ## Does not own
//! - Where the dump table bytes come from (`restore_authority` owns that)
//! - The authority check itself (`clio-compliance` owns that)
//!
//! ## Boundary
//!
//! Changes to how COPY text is parsed should normally require changes here.
//! Changes to how an archive is classified should not.
//!
//! Keep this module focused on the COPY text reader.

#[cfg(test)]
#[path = "restore_authority_copy_tests.rs"]
mod restore_authority_copy_tests;

use super::ClassifyRefusal;
use clio_types::{AmError, ErrorCode};
use std::io::BufRead;

/// The value of column `index` in every `COPY` text row that `reader` yields.
///
/// `COPY` text separates columns with a tab and rows with a newline, and
/// escapes a control character or a backslash with a leading backslash. `\N`
/// is SQL NULL and is skipped, and the `\.` end marker stops the read.
pub(super) fn copy_column<R: BufRead>(
    mut reader: R,
    index: usize,
) -> Result<Vec<String>, ClassifyRefusal> {
    let mut values = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).map_err(|error| {
            ClassifyRefusal::Io(AmError::new(
                ErrorCode::Internal,
                format!("reading a dump table member: {error}"),
            ))
        })?;
        if read == 0 {
            break;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line == b"\\." {
            break;
        }
        if let Some(value) = line.split(|&byte| byte == b'\t').nth(index)
            && value != b"\\N"
        {
            values.push(copy_unescape(value));
        }
    }
    Ok(values)
}

/// Undo the `COPY` text escapes in one value.
fn copy_unescape(value: &[u8]) -> String {
    let mut out = String::with_capacity(value.len());
    let mut bytes = value.iter().copied();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            out.push(char::from(byte));
            continue;
        }
        match bytes.next() {
            Some(b't') => out.push('\t'),
            Some(b'n') => out.push('\n'),
            Some(b'r') => out.push('\r'),
            Some(b'b') => out.push('\u{0008}'),
            Some(b'f') => out.push('\u{000c}'),
            Some(b'v') => out.push('\u{000b}'),
            Some(escaped) => out.push(char::from(escaped)),
            None => out.push('\\'),
        }
    }
    out
}
