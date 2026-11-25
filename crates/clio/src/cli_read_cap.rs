// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one byte cap every command puts on the
//! input document it reads from an injected stream, and the read that enforces
//! it.
//!
//! ## Owns
//! - `MAX_INPUT_BYTES`, the most bytes of an input document a command reads
//! - `read_document`, the capped read and the usage failures it reports: a
//!   document past the cap, and a stream that cannot be read as text
//!
//! ## Does not own
//! - Which flag names the document, or what the document must contain (see the
//!   command that reads it)
//! - Reading a document from a file path, which names a file the operator chose
//! - Parsing or validating the document's JSON (see `cli_write_batch` and
//!   `cli_write_hygiene`)
//!
//! ## Boundary
//!
//! Changes to the input cap, or to how a read past it fails, should normally
//! require changes here. Changes to one command's document format should not.
//!
//! Keep this module focused on the capped read of a command's input document.

use crate::cli_error::CliError;
use std::io::Read;

/// The most bytes of an input document a command reads from a stream. A larger
/// document is refused rather than buffered, so a writer this process does not
/// control cannot decide how much memory the process allocates.
pub(crate) const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// Read `reader` as the text of a command's input document, refusing a document
/// larger than [`MAX_INPUT_BYTES`].
///
/// `what` names the document in both messages (`batch --file -`), so a command
/// reports its own spelling of the input it read.
///
/// Allocation stops at the cap instead of at the end of the stream: the read
/// asks for at most one byte more than the cap, which is enough to tell a
/// document that fills the cap from one that passes it.
///
/// # Errors
/// A usage error (exit 2) when the stream cannot be read, when its bytes are not
/// UTF-8, or when the document is larger than the cap. The last one carries a
/// hint naming the alternative.
pub(crate) fn read_document(reader: &mut dyn Read, what: &str) -> Result<String, CliError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::usage(format!("{what} is not readable: {error}")))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(CliError::usage_hint(
            format!(
                "{what} is larger than the {} MiB input cap",
                MAX_INPUT_BYTES / (1024 * 1024)
            ),
            "split the document across more than one run",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| CliError::usage(format!("{what} is not readable: {error}")))
}

#[cfg(test)]
mod tests {
    use super::{MAX_INPUT_BYTES, read_document};
    use std::io::Cursor;
    use std::process::ExitCode;

    /// A reader whose every read fails, standing in for a closed pipe.
    struct Unreadable;

    impl std::io::Read for Unreadable {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("pipe closed"))
        }
    }

    fn mebibytes() -> String {
        format!("{} MiB", MAX_INPUT_BYTES / (1024 * 1024))
    }

    #[test]
    fn a_document_past_the_cap_is_refused_naming_the_cap_in_mib() {
        let mut reader = Cursor::new(vec![b'x'; MAX_INPUT_BYTES + 1]);
        let error = read_document(&mut reader, "`batch --file -`").expect_err("cap");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error.message().contains("`batch --file -` is larger than"),
            "{}",
            error.message()
        );
        assert!(
            error.message().contains(&mebibytes()),
            "{}",
            error.message()
        );
        assert_eq!(
            error.hint(),
            Some("split the document across more than one run")
        );
    }

    #[test]
    fn a_document_exactly_at_the_cap_is_read_whole() {
        let mut reader = Cursor::new(vec![b'y'; MAX_INPUT_BYTES]);
        let document = read_document(&mut reader, "`batch --file -`").expect("at the cap");
        assert_eq!(document.len(), MAX_INPUT_BYTES);
    }

    #[test]
    fn a_stream_that_cannot_be_read_is_not_readable() {
        let error =
            read_document(&mut Unreadable, "`hygiene clean --file -`").expect_err("read error");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error
                .message()
                .starts_with("`hygiene clean --file -` is not readable: "),
            "{}",
            error.message()
        );
    }

    #[test]
    fn bytes_that_are_not_utf8_are_not_readable() {
        let mut reader = Cursor::new(vec![0xff, 0xfe, 0xfd]);
        let error = read_document(&mut reader, "`batch --file -`").expect_err("not utf8");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error
                .message()
                .starts_with("`batch --file -` is not readable: "),
            "{}",
            error.message()
        );
    }
}
