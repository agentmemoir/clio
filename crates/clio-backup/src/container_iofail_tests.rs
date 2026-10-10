// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the container's io failure
//! mapping: every writer and reader io error surfaces as an Internal error
//! that names the failing step.
//!
//! ## Owns
//! - The failing reader/sink fixtures and the calibrated failure-point test
//!
//! ## Does not own
//! - Corruption or bounds tests (see the sibling `*_tests.rs` files)
//!
//! ## Boundary
//!
//! Keep this file focused on io failure surfaces.

use super::fixtures_tests::{MANIFEST_JSON, write_full_archive};
use super::{ArchiveReader, ArchiveWriter};
use clio_types::{AmError, ErrorCode};
use std::io::{Read, Write};

/// A sink that fails once its remaining-byte budget is exceeded, so io
/// failures surface at a calibrated point in the gzip stream.
pub(super) struct FailAfterBytes {
    remaining: usize,
}

impl Write for FailAfterBytes {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(std::io::Error::other("simulated sink failure"));
        }
        self.remaining -= buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A sink that logs the length of every inner write call, to calibrate the
/// failure points of `FailAfterBytes` against flate2's flush behavior.
struct LogWrites {
    log: Vec<usize>,
}

impl Write for LogWrites {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.log.push(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A member reader that always fails, to exercise io error mapping.
pub(super) struct FailRead;

impl Read for FailRead {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("simulated source failure"))
    }
}

/// A sink for `read_current` that always fails, to exercise io error
/// mapping on the read side.
pub(super) struct SinkFail;

impl Write for SinkFail {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("simulated sink failure"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn pseudo_random_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 16) as u8
        })
        .collect()
}

/// Every io failure on the writer path surfaces as an Internal error whose
/// message names the step, and the reader maps io failures the same way.
#[test]
fn t20_io_failures_surface_with_context() {
    let payload = pseudo_random_bytes(64 * 1024);

    // Calibrate: record the inner write calls of a good archive over this
    // sink. flate2 flushes the 10-byte gzip header during the first header
    // write and the rest at later dumps, so each failure point is reachable
    // by a byte budget.
    let mut probe = LogWrites { log: Vec::new() };
    {
        let mut w = ArchiveWriter::new(&mut probe);
        w.write_manifest(MANIFEST_JSON.as_bytes())
            .expect("manifest first");
        w.add_member("store.db", payload.as_slice())
            .expect("member");
        w.finish().expect("finish");
    }
    let total: usize = probe.log.iter().sum();
    let mut offsets = Vec::new();
    let mut acc = 0;
    for n in &probe.log {
        acc += n;
        offsets.push(acc);
    }
    assert_eq!(offsets[0], 10, "first inner write is the gzip header");
    assert!(offsets.len() >= 3, "this payload must flush mid-copy");

    // The gzip header flush fails inside the manifest header write.
    let mut w = ArchiveWriter::new(FailAfterBytes { remaining: 0 });
    let err = w
        .write_manifest(MANIFEST_JSON.as_bytes())
        .expect_err("first inner write fails");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        err.message.contains("writing the header of member"),
        "{}",
        err.message
    );

    // A flush during the payload copy-back fails with the member named.
    let mut w = ArchiveWriter::new(FailAfterBytes {
        remaining: offsets[1] - 1,
    });
    let err = (|| -> Result<(), AmError> {
        w.write_manifest(MANIFEST_JSON.as_bytes())?;
        w.add_member("store.db", payload.as_slice())?;
        w.finish()?;
        Ok(())
    })()
    .expect_err("mid-copy flush fails");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(err.message.contains("writing member"), "{}", err.message);

    // The gzip trailer write fails inside finish.
    let mut w = ArchiveWriter::new(FailAfterBytes {
        remaining: total - 1,
    });
    let err = (|| -> Result<(), AmError> {
        w.write_manifest(MANIFEST_JSON.as_bytes())?;
        w.add_member("store.db", payload.as_slice())?;
        w.finish()?;
        Ok(())
    })()
    .expect_err("trailer write fails");
    assert!(
        err.message.contains("finishing the archive gzip stream"),
        "{}",
        err.message
    );

    // A failing member source surfaces as "streaming member".
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    let err = w
        .add_member("store.db", FailRead)
        .expect_err("source read fails");
    assert!(err.message.contains("streaming member"), "{}", err.message);

    // The name gate runs before anything is written.
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    let err = w
        .add_member("../escape", &b"x"[..])
        .expect_err("gate rejects the name");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{}", err.message);

    // A failing write sink on the read side surfaces with the member named.
    let archive = write_full_archive();
    let mut r = ArchiveReader::new(archive.as_slice()).expect("opens");
    r.next_member().expect("manifest header");
    let err = r.read_current(&mut SinkFail).expect_err("sink write fails");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(err.message.contains("writing member"), "{}", err.message);

    // A failing source reader surfaces as an internal read failure.
    let mut r = ArchiveReader::new(FailRead).expect("opens");
    let err = r.next_member().expect_err("source read fails");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        err.message.contains("reading the archive stream"),
        "{}",
        err.message
    );
}

/// The spool creator reports a filesystem failure as Internal.
#[test]
fn t21_spool_create_failure_is_internal() {
    use super::spool::MemberSpool;

    let missing = std::env::temp_dir().join("clio-backup-missing-dir-for-spool-test");
    let Err(err) = MemberSpool::create(&missing, 1) else {
        panic!("parent directory does not exist")
    };
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(
        err.message.contains("creating a temporary spool file"),
        "{}",
        err.message
    );
}

/// A name collision retries on the next attempt suffix and still creates
/// the spool.
#[test]
fn t21b_spool_name_collision_retries() {
    use super::spool::MemberSpool;

    let dir = std::env::temp_dir();
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let first = MemberSpool::create(&dir, tag).expect("first spool");
    let second = MemberSpool::create(&dir, tag).expect("collision retries");
    drop(first);
    drop(second);
}
