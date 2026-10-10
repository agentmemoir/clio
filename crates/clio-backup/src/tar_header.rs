// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the 512-byte ustar header block: building
//! one for the writer and validating and decoding one for the reader.
//!
//! ## Owns
//! - The exact ustar header layout (field offsets, octal encoding, checksum)
//! - Header-level refusals: bad magic, bad version, non-file typeflag,
//!   checksum mismatch, malformed or binary size field
//!
//! ## Does not own
//! - Member-name rules (see `name`)
//! - The gzip wrapper, bounds, and member streaming (see `container`)
//!
//! ## Boundary
//!
//! Changes to header bytes should normally require changes here. Changes to
//! member ordering or bounds should not require changes here.
//!
//! Keep this module focused on the ustar header block.

use clio_types::{AmError, ErrorCode};

/// One ustar header block, in bytes.
pub(crate) const HEADER_SIZE: usize = 512;

/// The largest value the 11-octal-digit size field can encode
/// (8 GiB minus 1 byte); anything larger is refused by the writer.
pub(crate) const MAX_HEADER_SIZE_VALUE: u64 = 0o777_7777_7777;

const CHECKSUM_OFFSET: usize = 148;
const CHECKSUM_FIELD: usize = 8;
const TYPEFLAG_OFFSET: usize = 156;
const MAGIC_OFFSET: usize = 257;
const SIZE_OFFSET: usize = 124;
const SIZE_FIELD: usize = 12;

/// A decoded header: only the fields the container acts on.
pub(crate) struct DecodedHeader {
    pub(crate) size: u64,
}

/// Build the exact 512-byte header the writer emits.
///
/// mode 0o600, uid/gid 0, mtime 0, empty user/group names, typeflag `0`,
/// magic `ustar\0`, version `00`: pinned for byte-determinism.
///
/// `size` above [`MAX_HEADER_SIZE_VALUE`] is refused: the 12-byte field
/// carries 11 octal digits plus a NUL, so a larger value cannot be encoded
/// and must be an error, never a truncated or overflowed field.
///
/// The refusal is [`ErrorCode::OutOfRange`], not `QuotaExceeded` (#86). A
/// member larger than the size field is a property of the member and of the
/// container this build writes; no amount of free space makes an 8 GiB member
/// encodable. `QuotaExceeded` is the code a caller reads as "the machine has no
/// room", and the restore's classifier turns it into an out-of-space condition,
/// which sent the operator to free disk for a limit no disk could lift. The
/// reader's own caps follow the parallel rule: a cap is a property of the file,
/// so it is `ArchiveCorrupt` there.
pub(crate) fn build_header(name: &str, size: u64) -> Result<[u8; HEADER_SIZE], AmError> {
    if size > MAX_HEADER_SIZE_VALUE {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!(
                "member {name:?} is {size} bytes, over the ustar size-field limit of {MAX_HEADER_SIZE_VALUE} bytes; this archive format cannot represent a member this large, and no amount of free space would let it"
            ),
        ));
    }
    let mut block = [0u8; HEADER_SIZE];

    put_name(&mut block[0..100], name);
    put_octal(&mut block[100..108], 0o600); // mode
    put_octal(&mut block[108..116], 0); // uid
    put_octal(&mut block[116..124], 0); // gid
    put_octal(&mut block[SIZE_OFFSET..SIZE_OFFSET + SIZE_FIELD], size);
    put_octal(&mut block[136..148], 0); // mtime pinned to 0
    block[TYPEFLAG_OFFSET] = b'0';
    block[MAGIC_OFFSET..MAGIC_OFFSET + 6].copy_from_slice(b"ustar\0");
    block[MAGIC_OFFSET + 6..MAGIC_OFFSET + 8].copy_from_slice(b"00");
    // uname[32] at 265, gname[32] at 297 stay zero (empty names).
    put_octal(&mut block[329..337], 0); // devmajor
    put_octal(&mut block[337..345], 0); // devminor
    // prefix[155] at 345 stays zero: every name fits the 100-byte field.

    let checksum = unsigned_checksum(&block);
    let digits = format!("{checksum:06o}");
    block[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 6].copy_from_slice(digits.as_bytes());
    block[CHECKSUM_OFFSET + 6] = 0;
    block[CHECKSUM_OFFSET + 7] = b' ';
    Ok(block)
}

/// Store `value` right-aligned in zero-padded octal, NUL-terminated within
/// the field width (`field.len() - 1` digits).
///
/// The caller guarantees the value fits: `build_header` gates `size` against
/// [`MAX_HEADER_SIZE_VALUE`] before encoding, and every other value written
/// is a small constant.
fn put_octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
}

/// Store the name NUL-padded; a 100-byte name fills the field with no NUL,
/// which ustar allows.
fn put_name(field: &mut [u8], name: &str) {
    let bytes = name.as_bytes();
    field[..bytes.len()].copy_from_slice(bytes);
}

/// The unsigned checksum the writer produces and the reader verifies: the
/// sum of all 512 bytes with the checksum field read as eight spaces.
fn unsigned_checksum(block: &[u8; HEADER_SIZE]) -> u64 {
    block
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_FIELD).contains(&i) {
                u64::from(b' ')
            } else {
                u64::from(b)
            }
        })
        .sum()
}

/// Validate a header block and decode the fields the container acts on.
///
/// The caller has already validated the member name; refusals here are all
/// `ArchiveCorrupt` with the defect named.
pub(crate) fn parse_header(block: &[u8; HEADER_SIZE]) -> Result<DecodedHeader, AmError> {
    if &block[MAGIC_OFFSET..MAGIC_OFFSET + 6] != b"ustar\0" {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            "not a ustar header: the magic field is not \"ustar\\0\"",
        ));
    }
    if &block[MAGIC_OFFSET + 6..MAGIC_OFFSET + 8] != b"00" {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "unsupported ustar version {:?}; this build supports \"00\"",
                String::from_utf8_lossy(&block[MAGIC_OFFSET + 6..MAGIC_OFFSET + 8])
            ),
        ));
    }
    let typeflag = block[TYPEFLAG_OFFSET];
    if typeflag != b'0' {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "unsupported member type '{}'; this archive allows regular files only",
                char::from(typeflag)
            ),
        ));
    }

    let stored = parse_octal(&block[CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_FIELD])
        .ok_or_else(|| {
            AmError::new(
                ErrorCode::ArchiveCorrupt,
                "malformed ustar header: the checksum field is not octal",
            )
        })?;
    let computed = unsigned_checksum(block);
    if stored != computed {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!("ustar header checksum mismatch: stored {stored}, computed {computed}"),
        ));
    }

    let size = parse_octal(&block[SIZE_OFFSET..SIZE_OFFSET + SIZE_FIELD]).ok_or_else(|| {
        AmError::new(
            ErrorCode::ArchiveCorrupt,
            "malformed ustar header: the size field is not octal",
        )
    })?;

    Ok(DecodedHeader { size })
}

/// Read the NUL-terminated name out of the 100-byte name field. Exposed so
/// the container can validate the name before the rest of the header is
/// parsed.
pub(crate) fn peek_name(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// Parse an octal field that may be preceded by spaces and terminated by
/// NUL, space, or the field end. A leading high bit means the GNU binary
/// size encoding, which this build refuses.
fn parse_octal(field: &[u8]) -> Option<u64> {
    if (*field.first()? & 0x80) != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(field);
    let trimmed = text.trim_end_matches(['\0', ' ']).trim_start_matches(' ');
    if trimmed.is_empty() || !trimmed.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return None;
    }
    u64::from_str_radix(trimmed, 8).ok()
}
