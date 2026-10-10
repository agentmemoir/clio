// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the bounded archive reader: opening
//! members one at a time from the gzip-compressed ustar stream, enforcing
//! the reader bounds, and refusing damaged archives.
//!
//! ## Owns
//! - The `ArchiveReader` member-by-member state machine
//! - The end-of-archive marker rules (missing marker, data after it,
//!   truncated marker)
//! - The bound enforcement points: member count, declared member size
//!   before inflation, and total inflated size mid-stream
//! - Decode-path io error classification
//!
//! ## Does not own
//! - Member-name rules (see `name`)
//! - The 512-byte header format (see `tar_header`)
//! - The writer and the member spool (see `container`, `container_spool`)
//!
//! ## Boundary
//!
//! Changes to read behavior or bound enforcement should normally require
//! changes here. Changes to header bytes should not require changes here.
//!
//! Keep this module focused on the bounded archive reader.

use super::{MemberDigest, MemberHeader, ReaderBounds, io_internal, to_hex};
use crate::MANIFEST_MEMBER;
use crate::name::validate_member_name;
use crate::tar_header::{self, HEADER_SIZE};
use clio_types::{AmError, ErrorCode};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

/// Reads a gzip-compressed ustar archive member by member, with bounds.
pub struct ArchiveReader<R: Read> {
    inner: flate2::read::MultiGzDecoder<R>,
    state: ReaderState,
    members_seen: usize,
    total_bytes: u64,
    bounds: ReaderBounds,
    names: std::collections::BTreeSet<String>,
}

enum ReaderState {
    BetweenMembers,
    InMember {
        name: String,
        declared: u64,
        remaining: u64,
        padding: u64,
    },
    Finished,
}

impl<R: Read> ArchiveReader<R> {
    /// Open a reader with the default bounds.
    pub fn new(inner: R) -> Result<Self, AmError> {
        Self::with_bounds(inner, ReaderBounds::default())
    }

    /// Open a reader with explicit bounds.
    pub fn with_bounds(inner: R, bounds: ReaderBounds) -> Result<Self, AmError> {
        Ok(Self {
            inner: flate2::read::MultiGzDecoder::new(inner),
            state: ReaderState::BetweenMembers,
            members_seen: 0,
            total_bytes: 0,
            bounds,
            names: std::collections::BTreeSet::new(),
        })
    }

    /// Open the next member, or `None` after a clean end-of-archive marker.
    ///
    /// This never reads the payload: the declared size is checked against
    /// `max_member_bytes` before any member byte is inflated.
    pub fn next_member(&mut self) -> Result<Option<MemberHeader>, AmError> {
        if let ReaderState::InMember { name, .. } = &self.state {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                format!("member {name:?} is still open; read or skip it before the next member"),
            ));
        }
        if matches!(self.state, ReaderState::Finished) {
            return Ok(None);
        }

        let mut block = [0u8; HEADER_SIZE];
        let n = read_block(&mut self.inner, &mut block)?;
        if n == 0 {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                "missing end-of-archive marker: the stream ended where the two zero blocks belong",
            ));
        }
        if n < HEADER_SIZE {
            return Err(Self::block_tail_error(&block[..n]));
        }
        if block.iter().all(|&b| b == 0) {
            return self.finish_archive();
        }

        if self.members_seen >= self.bounds.max_members {
            // `ArchiveCorrupt`, not `QuotaExceeded`: a cap is a property of the
            // archive this build was handed, and the filesystem's own quota is a
            // property of the machine. One code for both sent the operator to
            // free disk space for an archive no amount of free space can read.
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "archive member count exceeds the cap of {} members",
                    self.bounds.max_members
                ),
            ));
        }
        let name = tar_header::peek_name(&block);
        if let Err(reason) = validate_member_name(&name) {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!("archive member name is not acceptable: {}", reason.message),
            ));
        }
        if self.members_seen == 0 && name != MANIFEST_MEMBER {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!("the first archive member must be {MANIFEST_MEMBER:?}, found {name:?}"),
            ));
        }
        // A name may appear once. The format has exactly one manifest, one
        // payload member, and at most one key member, so a repeat is not an
        // archive this build can produce, and it is not one it should read: two
        // members under one name leave every consumer free to pick either, and
        // for the key member there is no recorded digest to tell them apart.
        if !self.names.insert(name.clone()) {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!("the archive repeats the member name {name:?}"),
            ));
        }
        let decoded = tar_header::parse_header(&block)?;
        self.members_seen += 1;
        if decoded.size > self.bounds.max_member_bytes {
            // The same reason as the member-count cap above: a declared size
            // over the cap is this archive, not this machine.
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "member {name:?} declares {} bytes, over the per-member cap of {} bytes",
                    decoded.size, self.bounds.max_member_bytes
                ),
            ));
        }
        let padding = (HEADER_SIZE as u64 - decoded.size % HEADER_SIZE as u64) % HEADER_SIZE as u64;
        self.state = ReaderState::InMember {
            name: name.clone(),
            declared: decoded.size,
            remaining: decoded.size,
            padding,
        };
        Ok(Some(MemberHeader {
            name,
            size: decoded.size,
        }))
    }

    /// Stream the open member to `out`, returning its size and SHA-256.
    pub fn read_current(&mut self, out: &mut dyn Write) -> Result<MemberDigest, AmError> {
        let (size, sha256) = self.drain_current(Some(out))?;
        Ok(MemberDigest { size, sha256 })
    }

    /// Discard the open member, returning its size.
    pub fn skip_current(&mut self) -> Result<u64, AmError> {
        self.drain_current(None).map(|(size, _)| size)
    }

    /// Stream the remaining payload and padding, hashing the payload and
    /// enforcing the total-size cap mid-stream.
    fn drain_current(&mut self, mut out: Option<&mut dyn Write>) -> Result<(u64, String), AmError> {
        let ReaderState::InMember {
            name,
            declared,
            mut remaining,
            mut padding,
        } = std::mem::replace(&mut self.state, ReaderState::BetweenMembers)
        else {
            return Err(AmError::new(
                ErrorCode::InvalidArgument,
                "no member is open; call next_member first",
            ));
        };

        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        let mut streamed: u64 = 0;
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = self.read_decoded(&mut buf[..want])?;
            if n == 0 {
                return Err(AmError::new(
                    ErrorCode::ArchiveCorrupt,
                    format!(
                        "member {name:?} declared {declared} bytes but the stream ended after {streamed} bytes"
                    ),
                ));
            }
            if let Some(out) = out.as_mut() {
                out.write_all(&buf[..n])
                    .map_err(|e| io_internal(&format!("writing member {name:?}"), &e))?;
            }
            hasher.update(&buf[..n]);
            remaining -= n as u64;
            streamed += n as u64;
            self.total_bytes += n as u64;
            self.check_total()?;
        }
        while padding > 0 {
            let want = padding.min(buf.len() as u64) as usize;
            let n = self.read_decoded(&mut buf[..want])?;
            if n == 0 {
                return Err(AmError::new(
                    ErrorCode::ArchiveCorrupt,
                    format!("unexpected end of stream in the padding of member {name:?}"),
                ));
            }
            padding -= n as u64;
            self.total_bytes += n as u64;
            self.check_total()?;
        }
        Ok((streamed, to_hex(&hasher.finalize())))
    }

    /// Consume the second zero block of the end-of-archive marker.
    fn finish_archive(&mut self) -> Result<Option<MemberHeader>, AmError> {
        let mut second = [0u8; HEADER_SIZE];
        let n = read_block(&mut self.inner, &mut second)?;
        if n < HEADER_SIZE {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                "unexpected end of stream in the end-of-archive marker",
            ));
        }
        if second.iter().any(|&b| b != 0) {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                "unexpected data after the end-of-archive marker",
            ));
        }
        self.state = ReaderState::Finished;
        Ok(None)
    }

    /// A short read at a block boundary: a truncated end marker (all zero)
    /// or a truncated member header.
    fn block_tail_error(tail: &[u8]) -> AmError {
        if tail.iter().all(|&b| b == 0) {
            AmError::new(
                ErrorCode::ArchiveCorrupt,
                "unexpected end of stream in the end-of-archive marker",
            )
        } else {
            AmError::new(
                ErrorCode::ArchiveCorrupt,
                "unexpected end of stream in a member header block",
            )
        }
    }

    fn check_total(&self) -> Result<(), AmError> {
        if self.total_bytes > self.bounds.max_total_bytes {
            // The third cap, and the same reasoning as the other two.
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "archive members exceed the total inflated-size cap of {} bytes",
                    self.bounds.max_total_bytes
                ),
            ));
        }
        Ok(())
    }

    fn read_decoded(&mut self, buf: &mut [u8]) -> Result<usize, AmError> {
        self.inner.read(buf).map_err(|e| decode_error(&e))
    }
}

/// Read exactly `buf.len()` bytes, returning the number read before EOF.
fn read_block(reader: &mut dyn Read, buf: &mut [u8]) -> Result<usize, AmError> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader
            .read(&mut buf[filled..])
            .map_err(|e| decode_error(&e))?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// Classify a decode-path io error: a short or corrupt stream is archive
/// corruption; anything else is an internal failure.
fn decode_error(err: &std::io::Error) -> AmError {
    use std::io::ErrorKind::{InvalidData, InvalidInput, UnexpectedEof};
    if matches!(err.kind(), UnexpectedEof | InvalidData | InvalidInput) {
        AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!("the archive stream is truncated or corrupt: {err}"),
        )
    } else {
        io_internal("reading the archive stream", err)
    }
}
