// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bytes of the extract model: streaming one
//! HTTPS response, or one local file, into a temporary file beside the
//! destination and renaming it into place only when its length and its SHA-256
//! are the ones the caller expected.
//!
//! ## Owns
//! - The `ModelFetcher` seam and its real HTTP implementation
//! - The per-run temporary file, the length and digest checks, and the rename
//! - The local-copy install, which takes the same checks without a socket
//!
//! ## Does not own
//! - Whether a run needs the file at all, and where the stack says it goes (see
//!   `setup_extract_model`)
//! - The lines a run prints around a transfer (see `setup_extract_model`)
//!
//! ## Boundary
//!
//! Changes to how bytes reach the destination, or to what a refused transfer
//! leaves behind, should normally require changes here. Changes to when a
//! transfer runs should not.
//!
//! Keep this module on the transfer itself.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The read size for one chunk of the response body.
const CHUNK: usize = 64 * 1024;

/// How long a connection, and then the response headers, may take. The body has no
/// deadline of its own: a slow link is not a failure, and a stall is visible in the
/// progress lines.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The download seam, shaped like the `DockerRunner` seam: production passes
/// [`RealFetcher`], and a test passes a stub that writes its own bytes, so the
/// suite never opens a socket.
pub(crate) trait ModelFetcher {
    /// Stream `url` into `dest` by way of a temporary file beside it, reporting
    /// the running byte count after each chunk, and rename it into place only
    /// when its length and its SHA-256 are the expected ones. Returns the number
    /// of bytes written.
    ///
    /// `sha256` is the lowercase-hex digest the bytes must have. It is an
    /// argument rather than a constant so a caller — and a test — can verify a
    /// file other than the shipped one.
    ///
    /// # Errors
    /// A refused request, an answer that states no length, no bytes at all, a
    /// byte count or digest that differs from the expected one, or a filesystem
    /// failure. Any temporary file is removed before the error returns.
    fn fetch_to(
        &self,
        url: &str,
        dest: &Path,
        sha256: &str,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64, String>;
}

/// The real fetcher: one `ureq` agent per download, over the platform's trust
/// store.
pub(crate) struct RealFetcher;

impl ModelFetcher for RealFetcher {
    fn fetch_to(
        &self,
        url: &str,
        dest: &Path,
        sha256: &str,
        progress: &mut dyn FnMut(u64),
    ) -> Result<u64, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            // A non-2xx answer is a fact to report, not a transport failure, so the
            // status is read below rather than thrown here.
            .http_status_as_error(false)
            // The size check compares the bytes on disk with the advertised size,
            // and a compressed body would make `content-length` a length the file
            // never reaches.
            .accept_encoding("identity")
            .timeout_connect(Some(TIMEOUT))
            .timeout_recv_response(Some(TIMEOUT))
            // No deadline for the whole transfer: it is hundreds of megabytes, and a
            // healthy slow download must not be aborted for being slow.
            .timeout_global(None)
            .build()
            .into();
        let response = agent
            .get(url)
            .call()
            .map_err(|e| format!("the request failed: {e}"))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("the address answered status {status}"));
        }
        // The Hub's own length first, then the transport's. Either may be absent on
        // a chunked response, and an absent length is refused rather than trusted:
        // there would be nothing to check the body against.
        let expected = ["x-linked-size", "content-length"].iter().find_map(|name| {
            response
                .headers()
                .get(*name)?
                .to_str()
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        });
        let mut body = response.into_body().into_reader();
        stream_to_file(&mut body, dest, expected, sha256, progress)
    }
}

/// Install a local copy of the model instead of downloading it: the same length
/// and digest checks, the same temporary file, and the same rename.
///
/// # Errors
/// The source could not be read, it holds no bytes, its length or digest is not
/// the expected one, or the destination could not be written.
pub(crate) fn install_local(
    source: &Path,
    dest: &Path,
    sha256: &str,
    progress: &mut dyn FnMut(u64),
) -> Result<u64, String> {
    let mut file = std::fs::File::open(source)
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    let expected = file
        .metadata()
        .map(|meta| meta.len())
        .map_err(|e| format!("cannot read {}: {e}", source.display()))?;
    stream_to_file(&mut file, dest, Some(expected), sha256, progress)
}

/// A file that has not been renamed into place yet. Dropping it removes it, so
/// every error path between the first byte and the rename leaves no partial file
/// behind. It only ever holds a name this run created, so a drop can never delete
/// another run's file.
struct PartFile(PathBuf);

impl Drop for PartFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Stream `body` into `dest` through this run's temporary file beside it, check
/// the length and the digest, then rename it into place.
///
/// The destination's own directory is used for the temporary file, so the rename
/// is within one filesystem and either replaces the file or leaves the old one
/// alone.
fn stream_to_file(
    body: &mut dyn Read,
    dest: &Path,
    expected: Option<u64>,
    sha256: &str,
    progress: &mut dyn FnMut(u64),
) -> Result<u64, String> {
    let Some(expected) = expected else {
        return Err(
            "the answer stated no length, so the download was refused; run `clio setup` again"
                .to_owned(),
        );
    };
    let part = part_path(dest);
    // `create_new` rather than `create`: a name this process owns is one no other
    // run can be writing, and a collision is a refusal rather than a truncation of
    // someone else's file.
    let mut file = std::fs::File::create_new(&part)
        .map_err(|e| format!("cannot create {}: {e}", part.display()))?;
    // Only after the file is this run's own, so an error before this point cannot
    // remove a file this run did not create.
    let _partial = PartFile(part.clone());
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = body
            .read(&mut buffer)
            .map_err(|e| format!("the download stopped after {written} bytes: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        hasher.update(&buffer[..read]);
        written += read as u64;
        progress(written);
    }
    file.sync_all()
        .map_err(|e| format!("cannot flush {}: {e}", part.display()))?;
    drop(file);
    if written == 0 {
        return Err(
            "the answer carried no bytes, so the download was refused; run `clio setup` again"
                .to_owned(),
        );
    }
    if written != expected {
        return Err(format!(
            "the download ended after {written} of {expected} bytes, so the file was \
             discarded; run `clio setup` again"
        ));
    }
    let found = to_hex(&hasher.finalize());
    if found != sha256 {
        // A file already at the destination was never checked, and this run cannot
        // say what it holds, so it goes with the temporary file.
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "the file does not match the published checksum: expected {sha256}, found {found}, \
             so it was discarded; run `clio setup` again"
        ));
    }
    std::fs::rename(&part, dest).map_err(|e| {
        format!(
            "cannot move the download into place at {}: {e}",
            dest.display()
        )
    })?;
    Ok(written)
}

/// This run's half-written file: a sibling of `dest` on the same filesystem, so
/// the rename is one filesystem operation, named after the process that owns it so
/// two runs never write, rename, or delete the same file.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.part", std::process::id()));
    dest.with_file_name(name)
}

/// A digest as lowercase hex, the form the Hub publishes and the form a caller
/// passes back in.
fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
#[path = "setup_extract_model_stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
#[path = "setup_extract_model_http_tests.rs"]
mod http_tests;
