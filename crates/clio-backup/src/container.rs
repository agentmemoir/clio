// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the gzip-compressed ustar archive
//! container: the member writer, the public reader/writer types, the reader
//! bounds, and the shared container helpers.
//!
//! ## Owns
//! - The `ArchiveWriter` member order (manifest first, exactly once)
//! - The `MemberDigest` / `MemberHeader` / `ReaderBounds` public types and
//!   the `MAX_MANIFEST_BYTES` cap
//! - The gzip wrapper with the `mtime` pinned to zero
//! - The doc-hidden raw-member helper cross-crate tests use to hand-build an
//!   archive the writer correctly refuses
//! - The bounded reader implementation (see `container_reader`) and the
//!   member spool (see `container_spool`)
//!
//! ## Does not own
//! - Member-name validation (see `name`)
//! - Manifest fields or validation (see `manifest`)
//! - Database payloads or staging policy (see `clio-store`)
//!
//! ## Boundary
//!
//! Changes to container bytes or reader bounds should normally require changes
//! here. Changes to what a snapshot contains should not require changes here.
//!
//! Keep this module focused on the standard archive container.

use crate::MANIFEST_MEMBER;
use crate::name::validate_member_name;
use crate::tar_header::{self, HEADER_SIZE};
use clio_types::{AmError, ErrorCode};
use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

#[path = "container_reader.rs"]
mod reader;

#[path = "container_spool.rs"]
mod spool;

pub use reader::ArchiveReader;

use spool::MemberSpool;

/// A streamed member's size and lowercase-hex SHA-256 (64 characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberDigest {
    pub size: u64,
    pub sha256: String,
}

/// The name and declared size of the member `next_member` just opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberHeader {
    pub name: String,
    pub size: u64,
}

/// Caps a reader enforces so a hostile or damaged archive cannot exhaust
/// memory, time, or disk. Each cap has its own distinct error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaderBounds {
    pub max_members: usize,
    pub max_member_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for ReaderBounds {
    /// 1024 members is generous: one per table plus the manifest and the
    /// key store. `max_member_bytes` is 512 GiB, sized for local-store
    /// payloads. `max_total_bytes` is 1 TiB, sized for every member of one
    /// archive combined.
    fn default() -> Self {
        Self {
            max_members: 1024,
            max_member_bytes: 512 * 1024 * 1024 * 1024,
            max_total_bytes: 1024 * 1024 * 1024 * 1024,
        }
    }
}

/// The manifest stays a few kilobytes; a larger first member is refused.
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// True when a header's declared size is larger than the size the manifest
/// recorded for that member (T-07c): the archive grew behind the manifest's
/// back, so the payload cannot match the manifest digest.
#[must_use]
pub fn declared_size_suspect(declared: u64, manifest_recorded: u64) -> bool {
    declared > manifest_recorded
}

/// A gzip-compressed ustar archive built from raw (name, bytes) members, in
/// order, followed by the standard end-of-archive marker.
///
/// Test support, hidden from docs: a test in another crate must hand-build an
/// archive this crate's writer correctly refuses (a repeated member name), and
/// duplicating the header format there would let the copy drift from the one
/// this crate owns. The writer's rules do not apply; the caller chooses the
/// names and the order.
#[doc(hidden)]
pub fn archive_from_raw_members_for_tests(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (name, payload) in members {
        let header = tar_header::build_header(name, payload.len() as u64)
            .expect("a test member name the ustar header accepts");
        raw.extend_from_slice(&header);
        raw.extend_from_slice(payload);
        let pad = HEADER_SIZE - (payload.len() % HEADER_SIZE);
        if pad < HEADER_SIZE {
            raw.extend((0..pad).map(|_| 0u8));
        }
    }
    raw.extend_from_slice(&[0u8; 2 * HEADER_SIZE]);
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    encoder
        .write_all(&raw)
        .expect("gzip the hand-built archive");
    encoder.finish().expect("finish the gzip stream")
}

/// Forwards writes to a file while hashing and counting the bytes, so the
/// size and digest are known before the header is built.
struct HashWriter<'a> {
    file: &'a mut std::fs::File,
    hasher: &'a mut Sha256,
    written: u64,
}

impl Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write_all(buf)?;
        self.hasher.update(buf);
        self.written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Writes a gzip-compressed ustar archive: the manifest first, then payload
/// members, then the end-of-archive marker.
pub struct ArchiveWriter<W: Write> {
    inner: GzEncoder<W>,
    manifest_written: bool,
    names: std::collections::BTreeSet<String>,
}

impl<W: Write> ArchiveWriter<W> {
    /// Wrap `inner` in a gzip stream with `mtime` 0 and no file name, so
    /// identical archives compress to identical bytes.
    pub fn new(inner: W) -> Self {
        Self {
            inner: GzBuilder::new()
                .mtime(0)
                .write(inner, Compression::default()),
            manifest_written: false,
            names: std::collections::BTreeSet::new(),
        }
    }

    /// Write the manifest as member 0. Refused if a manifest was already
    /// written.
    pub fn write_manifest(&mut self, manifest_json_bytes: &[u8]) -> Result<MemberDigest, AmError> {
        if self.manifest_written {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "the manifest has already been written; an archive carries exactly one",
            ));
        }
        self.manifest_written = true;
        self.names.insert(MANIFEST_MEMBER.to_owned());
        self.write_spooled(MANIFEST_MEMBER, manifest_json_bytes)
    }

    /// Stream the next payload member. Refused before the manifest is
    /// written, for the reserved manifest member name, and for a name this
    /// archive already carries.
    pub fn add_member(&mut self, name: &str, reader: impl Read) -> Result<MemberDigest, AmError> {
        self.guard_payload_name(name)?;
        self.write_spooled(name, reader)
    }

    /// Stream a payload member whose size is already known.
    ///
    /// A staged member was measured while it was staged, so the caller hands
    /// that size in and the writer skips the spool: the member bytes are read
    /// once, never copied to a scratch file, and never hashed a second time.
    /// A size that disagrees with what the reader produces is refused rather
    /// than written as a broken member.
    pub fn add_measured_member(
        &mut self,
        name: &str,
        mut reader: impl Read,
        size: u64,
    ) -> Result<(), AmError> {
        self.guard_payload_name(name)?;
        self.write_premeasured(name, &mut reader, size)
    }

    /// Write a member of a known size: header, payload, and NUL padding to the
    /// 512-byte multiple.
    fn write_premeasured(
        &mut self,
        name: &str,
        reader: &mut impl Read,
        size: u64,
    ) -> Result<(), AmError> {
        let header = tar_header::build_header(name, size)?;
        self.write_block(&header, &format!("writing the header of member {name:?}"))?;
        let copied = std::io::copy(reader, &mut self.inner)
            .map_err(|e| io_internal(&format!("writing member {name:?}"), &e))?;
        if copied != size {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("member {name:?} was measured as {size} bytes but produced {copied}"),
            ));
        }
        let pad = (HEADER_SIZE as u64 - size % HEADER_SIZE as u64) % HEADER_SIZE as u64;
        if pad > 0 {
            let zeros = [0u8; HEADER_SIZE - 1];
            self.write_block(&zeros[..pad as usize], &format!("padding member {name:?}"))?;
        }
        Ok(())
    }

    /// The member-order rules every payload member passes: after the manifest,
    /// never under the manifest's reserved name, and never under a name this
    /// archive already carries.
    ///
    /// The once-only rule is the writer's side of the same rule the reader
    /// enforces: a name may appear once, so a second member under it is refused
    /// here rather than written and then refused on the way back in.
    fn guard_payload_name(&mut self, name: &str) -> Result<(), AmError> {
        validate_member_name(name)?;
        if !self.manifest_written {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "the manifest must be the first member",
            ));
        }
        if name == MANIFEST_MEMBER {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("member name {MANIFEST_MEMBER:?} is reserved for the manifest"),
            ));
        }
        if !self.names.insert(name.to_owned()) {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("the archive already carries a member named {name:?}; a name appears once"),
            ));
        }
        Ok(())
    }

    /// Spool the member to measure its size and digest, then write it.
    ///
    /// The name is validated before a byte is spooled, so an illegal name is
    /// refused without touching the spool or the archive.
    fn write_spooled(
        &mut self,
        name: &str,
        mut reader: impl Read,
    ) -> Result<MemberDigest, AmError> {
        validate_member_name(name)?;
        // The tag only has to make the spool name unique, so a clock before the
        // epoch contributes 0 and the pid and attempt suffix still separate two
        // spools. This read is a name, not a timestamp: it never panics and never
        // fails, the same rule every other clock read in this crate follows.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let mut spool = MemberSpool::create(&std::env::temp_dir(), nanos)?;
        let mut hasher = Sha256::new();
        let size;
        {
            let mut hash_file = HashWriter {
                file: &mut spool.file,
                hasher: &mut hasher,
                written: 0,
            };
            std::io::copy(&mut reader, &mut hash_file)
                .map_err(|e| io_internal(&format!("streaming member {name:?}"), &e))?;
            size = hash_file.written;
        }
        spool.rewind();
        self.write_premeasured(name, &mut spool, size)?;
        Ok(MemberDigest {
            size,
            sha256: to_hex(&hasher.finalize()),
        })
    }

    /// Write bytes through the gzip encoder, naming the step in io errors.
    fn write_block(&mut self, bytes: &[u8], context: &str) -> Result<(), AmError> {
        self.inner
            .write_all(bytes)
            .map_err(|e| io_internal(context, &e))
    }

    /// End the archive: two 512-byte zero blocks, then the gzip trailer.
    pub fn finish(mut self) -> Result<W, AmError> {
        self.write_block(&[0u8; 2 * HEADER_SIZE], "writing the end-of-archive marker")?;
        self.inner
            .finish()
            .map_err(|e| io_internal("finishing the archive gzip stream", &e))
    }
}

/// Encode bytes as lowercase hex (64 characters for a SHA-256 digest).
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

/// Wrap an io error as an internal failure, keeping the io error text.
pub(crate) fn io_internal(context: &str, err: &std::io::Error) -> AmError {
    AmError::new(ErrorCode::Internal, format!("{context}: {err}"))
}

#[cfg(test)]
#[path = "container_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "container_fixtures_tests.rs"]
mod fixtures_tests;

#[cfg(test)]
#[path = "container_interop_tests.rs"]
mod interop_tests;

#[cfg(test)]
#[path = "container_corrupt_tests.rs"]
mod corrupt_tests;

#[cfg(test)]
#[path = "container_iofail_tests.rs"]
mod iofail_tests;
