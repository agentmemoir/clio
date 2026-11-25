// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two caps every command puts on the input
//! document it reads: the bytes it will buffer, and the entries it will hold in
//! memory once the document is parsed.
//!
//! ## Owns
//! - `MAX_INPUT_BYTES`, the most bytes of an input document a command reads
//! - `MAX_INPUT_ENTRIES`, the most entries one document may carry
//! - `read_document`, the capped read of any reader and the usage failures it
//!   reports: a document past the cap, and a stream that cannot be read as text
//! - `read_document_file`, the same capped read for a named path, so a FIFO or
//!   `/dev/stdin` is capped exactly like the injected stream
//!
//! ## Does not own
//! - Which flag names the document, or what the document must contain (see the
//!   command that reads it)
//! - Parsing or validating the document's JSON (see `cli_write_batch` and
//!   `cli_write_hygiene`)
//! - The per-command bound on entries tighter than `MAX_INPUT_ENTRIES` (see the
//!   atomic batch's own operation cap)
//!
//! ## Boundary
//!
//! Changes to either input cap, or to how a read past one fails, should normally
//! require changes here. Changes to one command's document format should not.
//!
//! Keep this module focused on the capped read of a command's input document.

use crate::cli_error::CliError;
use std::io::Read;

/// The most bytes of an input document a command reads. A larger document is
/// refused rather than buffered, so a writer this process does not control
/// cannot decide how much memory the process allocates.
///
/// The number is chosen from measurement. A legitimately largest document is a
/// few hundred KiB: 50 store operations at the largest `gist` and `context` the
/// schema permits measure 223 KiB, and the same 50 with a 2 KB span haystack
/// each measure 329 KiB. A document of many tiny entries at exactly this size
/// peaks near 205 MiB, because a parsed JSON tree costs many times the bytes
/// that spelled it; at the 8 MiB this cap used to be, the same shape peaked near
/// 772 MiB. Four MiB keeps a measured peak in the low hundreds of MiB while
/// leaving more than twelve times the largest legitimate document.
pub(crate) const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;

/// The most entries one input document may carry, across both commands.
///
/// The byte cap bounds the text; this bounds the parsed tree and the copies of
/// it a command builds, which is where a document of many tiny entries spends
/// its memory. It is set at the audit scan budget, so no document the product's
/// own report can produce is ever refused; a legitimate page is a few thousand
/// candidates, so the headroom is at least three times and in practice far more.
/// The atomic batch is bounded tighter still by its own operation cap.
pub(crate) const MAX_INPUT_ENTRIES: usize = 10_000;

/// Read `reader` as the text of a command's input document, refusing a document
/// larger than [`MAX_INPUT_BYTES`].
///
/// `what` names the document in both messages (`batch --file -`), so a command
/// reports its own spelling of the input it read. `hint` is the alternative that
/// command can honestly offer a caller whose document is too large.
///
/// Allocation stops at the cap instead of at the end of the stream: the read
/// asks for at most one byte more than the cap, which is enough to tell a
/// document that fills the cap from one that passes it.
///
/// # Errors
/// A usage error (exit 2) when the stream cannot be read, when its bytes are not
/// UTF-8, or when the document is larger than the cap. The last one carries the
/// caller's hint naming the alternative.
pub(crate) fn read_document(
    reader: &mut dyn Read,
    what: &str,
    hint: &str,
) -> Result<String, CliError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::usage(format!("{what} is not readable: {error}")))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(too_large(what, hint));
    }
    String::from_utf8(bytes)
        .map_err(|error| CliError::usage(format!("{what} is not readable: {error}")))
}

/// Read the input document named by `path` through [`read_document`], so a FIFO,
/// `/dev/stdin`, or an oversized regular file is refused at the same cap as an
/// injected stream instead of being read to its end.
///
/// `what` names the document in both messages, exactly as for the stream form.
///
/// # Errors
/// A usage error (exit 2) when the path cannot be opened, and the failures
/// [`read_document`] reports.
pub(crate) fn read_document_file(path: &str, what: &str, hint: &str) -> Result<String, CliError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| CliError::usage(format!("{what} is not readable: {error}")))?;
    read_document(&mut file, what, hint)
}

/// The refusal for a document past [`MAX_INPUT_BYTES`], naming the cap in MiB.
fn too_large(what: &str, hint: &str) -> CliError {
    CliError::usage_hint(
        format!(
            "{what} is larger than the {} MiB input cap",
            MAX_INPUT_BYTES / (1024 * 1024)
        ),
        hint,
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_INPUT_BYTES, MAX_INPUT_ENTRIES, read_document, read_document_file};
    use std::io::Cursor;
    use std::process::ExitCode;

    /// The hint a command passes for a document with independent entries.
    const HINT: &str = "split the document across more than one run";

    /// A reader whose every read fails, standing in for a closed pipe.
    struct Unreadable;

    impl std::io::Read for Unreadable {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("pipe closed"))
        }
    }

    /// One gibibyte, the size of the source the bounded read is proven against:
    /// far more than this process could ever buffer in a test.
    const SOURCE_BYTES: usize = 1024 * 1024 * 1024;

    /// A reader that can supply [`SOURCE_BYTES`] without holding them, and that
    /// counts every byte it has handed out. The count, not the wall clock and
    /// not the size of the result, is the evidence that the read stopped at the
    /// cap rather than at the end of the stream.
    struct CountingSource {
        remaining: usize,
        served: usize,
    }

    impl CountingSource {
        /// A source of one gibibyte that nothing has been read from yet.
        fn gigabytes() -> Self {
            Self {
                remaining: SOURCE_BYTES,
                served: 0,
            }
        }
    }

    impl std::io::Read for CountingSource {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let served = buf.len().min(self.remaining);
            buf[..served].fill(b'x');
            self.remaining -= served;
            self.served += served;
            Ok(served)
        }
    }

    /// More slack than any pipe buffer holds: a writer sees at most the pipe's
    /// own capacity of bytes the reader never took, so a read that stops at the
    /// cap shows up as a writer count just past the cap, while a read that runs
    /// to the end of the stream shows up as a gibibyte.
    const PIPE_SLACK: u64 = 1024 * 1024;

    fn mebibytes() -> String {
        format!("{} MiB", MAX_INPUT_BYTES / (1024 * 1024))
    }

    #[test]
    fn recorded_input_caps_are_pinned() {
        // These two numbers are the recorded security posture: the changelog and
        // both commands' refusal text are written against them, so raising one
        // is a defect, not a tuning choice. A deliberate change edits this test
        // and the changelog together.
        assert_eq!(
            MAX_INPUT_BYTES,
            4 * 1024 * 1024,
            "a raised input byte cap is a defect, not a tuning choice"
        );
        assert_eq!(
            MAX_INPUT_ENTRIES, 10_000,
            "a raised input entry cap is a defect, not a tuning choice"
        );
    }

    #[test]
    fn a_source_past_the_cap_is_read_only_one_byte_past_the_cap() {
        let mut source = CountingSource::gigabytes();
        let error = read_document(&mut source, "`batch --file -`", HINT)
            .expect_err("a gibibyte source is past the cap");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error.message().contains("is larger than the"),
            "{}",
            error.message()
        );
        // Exactly one byte past the cap: enough to refuse, and the whole of what
        // the read takes. Draining the stream first would show SOURCE_BYTES.
        assert_eq!(
            source.served,
            MAX_INPUT_BYTES + 1,
            "the read must stop one byte past the cap, not at the end of the stream"
        );
        assert!(
            source.remaining > 0,
            "the source must still hold bytes, or nothing was proven"
        );
    }

    #[test]
    fn a_realistically_large_page_is_well_inside_the_entry_cap() {
        // The largest page a legitimate document carries is a few thousand
        // candidates; the entry cap must leave clear headroom over it.
        let page = (0..3_000)
            .map(|i| format!("{{\"item_id\":\"itm-{i}\"}}"))
            .collect::<Vec<_>>()
            .join(",");
        let entries = page.matches("item_id").count();
        assert!(
            entries * 3 < MAX_INPUT_ENTRIES,
            "{entries} entries against a cap of {MAX_INPUT_ENTRIES}"
        );
    }

    #[test]
    fn a_document_past_the_cap_is_refused_naming_the_cap_in_mib() {
        let mut reader = Cursor::new(vec![b'x'; MAX_INPUT_BYTES + 1]);
        let error =
            read_document(&mut reader, "`batch --file -`", HINT).expect_err("cap is exceeded");
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
        assert_eq!(error.hint(), Some(HINT));
    }

    #[test]
    fn the_hint_is_the_callers_own_alternative() {
        let mut reader = Cursor::new(vec![b'x'; MAX_INPUT_BYTES + 1]);
        let error = read_document(&mut reader, "`batch --file -`", "send fewer operations")
            .expect_err("cap is exceeded");
        assert_eq!(error.hint(), Some("send fewer operations"));
    }

    #[test]
    fn a_document_exactly_at_the_cap_is_read_whole() {
        let mut reader = Cursor::new(vec![b'y'; MAX_INPUT_BYTES]);
        let document =
            read_document(&mut reader, "`batch --file -`", HINT).expect("a document at the cap");
        assert_eq!(document.len(), MAX_INPUT_BYTES);
    }

    #[test]
    fn a_stream_that_cannot_be_read_is_not_readable() {
        let error = read_document(&mut Unreadable, "`hygiene clean --file -`", HINT)
            .expect_err("read error");
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
        let error = read_document(&mut reader, "`batch --file -`", HINT).expect_err("not utf8");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error
                .message()
                .starts_with("`batch --file -` is not readable: "),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_missing_path_is_not_readable_and_names_the_path() {
        let error = read_document_file(
            "/nonexistent/ops.json",
            "--file `/nonexistent/ops.json`",
            HINT,
        )
        .expect_err("missing path");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error
                .message()
                .starts_with("--file `/nonexistent/ops.json` is not readable: "),
            "{}",
            error.message()
        );
    }

    #[test]
    fn an_oversized_file_is_refused_at_the_cap() {
        let path = std::env::temp_dir().join(format!(
            "clio-read-cap-oversized-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, vec![b'z'; MAX_INPUT_BYTES + 2]).expect("write the oversized file");
        let shown = format!("--file `{}`", path.display());
        let error =
            read_document_file(&path.to_string_lossy(), &shown, HINT).expect_err("oversized file");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error.message().contains("is larger than the"),
            "{}",
            error.message()
        );
        assert!(
            error.message().contains(&mebibytes()),
            "{}",
            error.message()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_exactly_at_the_cap_is_read_whole() {
        // The path form's accept boundary, paired with the refusals above: a
        // document that fills the cap is delivered entire rather than truncated
        // or refused, so the cap costs no legitimate document its last byte.
        let path =
            std::env::temp_dir().join(format!("clio-read-cap-at-cap-{}.json", std::process::id()));
        let mut body = vec![b'q'; MAX_INPUT_BYTES];
        body[MAX_INPUT_BYTES - 1] = b'\n';
        std::fs::write(&path, &body).expect("write the file at the cap");
        let shown = format!("--file `{}`", path.display());
        let document =
            read_document_file(&path.to_string_lossy(), &shown, HINT).expect("a file at the cap");
        assert_eq!(document.len(), MAX_INPUT_BYTES, "the file is not truncated");
        assert!(document.ends_with('\n'), "the last byte is delivered");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_fifo_path_is_refused_at_the_cap_instead_of_read_to_its_end() {
        let Some(path) = fifo() else {
            return;
        };
        // The writer counts what it manages to publish, which is the only
        // evidence the path form has that the read stopped at the cap: the file
        // is opened inside `read_document_file`, so no counting reader can be
        // interposed. A read that runs to the end of the stream drains the whole
        // source and shows up as many hundreds of MiB written.
        let writer = std::thread::spawn({
            let path = path.clone();
            move || {
                use std::io::Write as _;
                let mut pipe = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .expect("open the fifo for writing");
                let chunk = vec![b'w'; 64 * 1024];
                let mut written = 0_u64;
                while let Ok(count) = pipe.write(&chunk) {
                    if count == 0 {
                        break;
                    }
                    written += count as u64;
                }
                written
            }
        });
        let error = read_document_file(&path.to_string_lossy(), "`batch --file -`", HINT)
            .expect_err("a fifo past the cap");
        assert_eq!(error.exit_code(), ExitCode::from(2));
        assert!(
            error.message().contains("is larger than the"),
            "{}",
            error.message()
        );
        let written = writer.join().expect("the writer ends when the fifo closes");
        assert!(
            written > MAX_INPUT_BYTES as u64,
            "the writer must have supplied the bytes the cap consumed: {written}"
        );
        assert!(
            written < MAX_INPUT_BYTES as u64 + PIPE_SLACK,
            "the read stopped at the cap, so only buffered bytes sit unread: {written}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Create a FIFO at a unique temp path, or `None` where the host has no
    /// `mkfifo` (the test is then vacuous rather than failing on the platform).
    fn fifo() -> Option<std::path::PathBuf> {
        let path =
            std::env::temp_dir().join(format!("clio-read-cap-fifo-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .ok()?;
        status.success().then_some(path)
    }
}
