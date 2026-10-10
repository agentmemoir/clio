// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one snapshot's name: the archive file name,
//! the snapshot id it is built from, and the UTC clock both are stamped with.
//!
//! ## Owns
//! - The `<slug>-<YYYYMMDDTHHMMSSZ>-<8 hex>` id and the file name around it
//! - `collision_suffix`, the 8 hex characters that keep two creates in the
//!   same second apart
//! - The compact and RFC 3339 UTC forms, and the id a file name carries
//!
//! ## Does not own
//! - The slug itself (see `clio-config`'s `store_slug`)
//! - Directory resolution (see `clio-config`'s `backup_path`)
//! - Writing or reading the archive (see `create`, `inventory`)
//!
//! ## Boundary
//!
//! Changes to the archive name pattern should normally require changes here.
//! Changes to the archive's contents should not.
//!
//! Keep this module focused on snapshot naming.

use crate::SNAPSHOT_EXTENSION;
use clio_types::{AmError, ErrorCode, civil_from_days};
use sha2::{Digest, Sha256};

/// Hex characters in the collision suffix.
const SUFFIX_HEX: usize = 8;

/// The snapshot id: `<slug>-<YYYYMMDDTHHMMSSZ>-<8 hex>`.
///
/// The id is the archive's file-name stem. It is never a database row id, and
/// the slug it starts from is credential-free by construction, so a database
/// URL with a password cannot reach a name.
#[must_use]
pub fn snapshot_id(slug: &str, seconds: u64, suffix: &str) -> String {
    format!("{slug}-{}-{}", compact_utc(seconds), suffix)
}

/// The archive file name for a snapshot id.
#[must_use]
pub fn archive_file_name(snapshot_id: &str) -> String {
    format!("{snapshot_id}{SNAPSHOT_EXTENSION}")
}

/// The snapshot id an archive file name carries, when it carries one.
///
/// `None` for any file that is not a snapshot archive, so a caller never treats
/// an unrelated file as a snapshot.
#[must_use]
pub fn snapshot_id_of(path: &std::path::Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    name.strip_suffix(SNAPSHOT_EXTENSION)
        .filter(|stem| !stem.is_empty())
        .map(str::to_owned)
}

/// Eight hex characters that keep two creates in the same second apart.
///
/// Derived from the process id, the nanosecond clock, and a process-local
/// counter, so two creates in one second — in one process or two — never
/// collide. The value is a collision-avoidance tag, not a secret: it guards a
/// file name, and nothing is authenticated with it.
///
/// # Panics
/// Never: the clock and the counter are both total functions.
#[must_use]
pub fn collision_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!("{}:{nanos}:{sequence}", std::process::id());
    let digest = Sha256::digest(seed.as_bytes());
    crate::container::to_hex(&digest[..SUFFIX_HEX / 2])
}

/// UTC `YYYYMMDDTHHMMSSZ`: the compact stamp inside a snapshot id.
#[must_use]
pub fn compact_utc(seconds: u64) -> String {
    let (year, month, day, hour, minute, second) = parts(seconds);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// UTC `YYYY-MM-DDThh:mm:ssZ`: the manifest's `created_at` form.
#[must_use]
pub fn rfc3339_utc(seconds: u64) -> String {
    let (year, month, day, hour, minute, second) = parts(seconds);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The current instant in whole seconds since the epoch.
#[must_use]
pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Split a UTC instant into `(year, month, day, hour, minute, second)`.
fn parts(seconds: u64) -> (i64, u32, u32, u64, u64, u64) {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let (year, month, day) = civil_from_days(days);
    let rest = seconds % 86_400;
    (year, month, day, rest / 3600, (rest % 3600) / 60, rest % 60)
}

/// Refuse a slug that would put a separator or an unsafe byte in a file name.
///
/// The slug arrives from a resolver that already sanitizes it; this is the
/// second gate, so a name can never escape the backup directory even if a slug
/// source changes. The allowed set is printable ASCII without a space, so a
/// quoted name is always one word.
///
/// # Errors
/// The slug is empty, longer than the name allows, or holds a separator, a
/// space, a control character, or a non-ASCII byte.
pub fn check_slug(slug: &str) -> Result<(), AmError> {
    let reject = |reason: &str| {
        Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("store slug {slug:?} cannot name a snapshot: {reason}"),
        ))
    };
    if slug.is_empty() {
        return reject("it is empty");
    }
    if slug.len() > 64 {
        return reject("it is longer than 64 bytes");
    }
    if let Some(&byte) = slug
        .as_bytes()
        .iter()
        .find(|&&byte| !(0x21..=0x7E).contains(&byte) || byte == b'/' || byte == b'\\')
    {
        return reject(&format!("byte 0x{byte:02x} is not a safe name character"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive_name_tests.rs"]
mod tests;
