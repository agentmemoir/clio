// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the staging discipline: a staged
//! member's digest and size, its removal on drop, the owner-only mode, the
//! refusal cases, and the removal of the staged file on every path out of a copy
//! that fails.
//!
//! ## Owns
//! - The staging assertions, including the mid-stream write and flush failures
//!   the staging loop maps to errors
//! - The code a full or over-quota filesystem is reported as, and the codes that
//!   must stay an internal fault
//!
//! ## Does not own
//! - The atomic publish (see `atomic_publish_tests`)
//! - Create, verify, or inventory cases (see their own test modules)
//!
//! ## Boundary
//!
//! Keep this file focused on staging one member.

use super::*;
use crate::fixtures::{Scratch, has_extension, names_with_extension};
use std::io::{Read, Write};

/// The `.tmp` files left in `dir` after a staging call, which is what these
/// tests assert is empty.
fn staged_temporaries(dir: &Path) -> Vec<String> {
    names_with_extension(dir, "tmp")
}

#[test]
fn a_staged_copy_reports_its_size_and_digest_and_is_removed_on_drop() {
    let scratch = Scratch::new("staging-copy");
    let source = scratch.join("source.bin");
    let bytes: Vec<u8> = (0..5000u32).map(|index| (index % 251) as u8).collect();
    std::fs::write(&source, &bytes).expect("write the source");

    let (staged_bytes, path) = {
        let staged = stage_copy(&source, scratch.path(), "member").expect("stage");
        assert_eq!(staged.bytes(), bytes.len() as u64);
        assert_eq!(staged.sha256().len(), 64);
        assert!(staged.path().exists());
        (staged.bytes(), staged.path().to_path_buf())
    };

    assert_eq!(staged_bytes, 5000);
    assert!(!path.exists(), "the staged member is removed on drop");
}

#[test]
fn a_staged_copy_matches_an_independently_computed_digest() {
    let scratch = Scratch::new("staging-digest");
    let source = scratch.join("source.bin");
    std::fs::write(&source, b"clio snapshot payload").expect("write the source");

    let staged = stage_copy(&source, scratch.path(), "member").expect("stage");

    let expected = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(b"clio snapshot payload");
        let mut hex = String::new();
        for byte in &digest {
            let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
        }
        hex
    };
    assert_eq!(staged.sha256(), expected);
}

#[cfg(unix)]
#[test]
fn a_staged_member_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("staging-mode");
    let source = scratch.join("source.bin");
    std::fs::write(&source, b"key material").expect("write the source");

    let staged = stage_copy(&source, scratch.path(), "member").expect("stage");

    let mode = std::fs::metadata(staged.path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn a_reserved_path_does_not_exist_yet() {
    let scratch = Scratch::new("staging-reserve");

    let path = reserve_path(scratch.path(), "payload").expect("reserve");

    assert!(
        !path.exists(),
        "VACUUM INTO needs a path that does not exist"
    );
    assert!(path.starts_with(scratch.path()));
    // A second reservation in the same instant gets a different name.
    let again = reserve_path(scratch.path(), "payload").expect("reserve again");
    assert_ne!(path, again);
}

#[test]
fn a_missing_source_is_an_error_and_stages_nothing() {
    let scratch = Scratch::new("staging-missing");

    let error = stage_copy(&scratch.join("absent"), scratch.path(), "member")
        .expect_err("an absent source cannot be staged");

    assert_eq!(error.code, clio_types::ErrorCode::Internal);
    let temporaries: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| has_extension(name, "tmp"))
        .collect();
    assert!(temporaries.is_empty(), "{temporaries:?}");
}

#[test]
fn the_backup_directory_is_created_owner_only_and_left_alone_when_present() {
    let scratch = Scratch::new("dir-create");
    let fresh = scratch.join("backups");

    assert!(ensure_backup_dir(&fresh).expect("create"));
    assert!(fresh.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    assert!(
        !ensure_backup_dir(&fresh).expect("second call"),
        "already there"
    );
}

#[test]
fn begin_and_adopt_staged_bracket_a_stream() {
    let scratch = Scratch::new("staging-begin");
    let (path, mut file) = begin_staged(scratch.path(), "stream").expect("begin");
    assert!(path.exists());
    file.write_all(b"streamed bytes").expect("write");
    file.sync_all().expect("flush");
    drop(file);

    let staged = adopt_staged(path.clone(), 14, "d".repeat(64));
    assert_eq!(staged.bytes(), 14);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 14);
    drop(staged);
    assert!(!path.exists());
}

/// A guard that was never released removes the staged file, which is the whole
/// point of wrapping the path: a copy that fails cannot leave the file behind.
#[test]
fn an_unreleased_staged_path_is_removed_when_the_guard_drops() {
    let scratch = Scratch::new("staging-guard-drop");
    let path = reserve_path(scratch.path(), "member").expect("reserve");
    std::fs::write(&path, b"key material").expect("write the staged bytes");
    assert!(path.exists());

    drop(StagedGuard::new(path.clone()));

    assert!(
        !path.exists(),
        "a guard that never released removes its file"
    );
}

/// A released path survives the guard, so the returned `StagedFile` — and only
/// it — removes the file, once, when the run ends.
#[test]
fn a_released_staged_path_survives_the_guard_and_is_removed_by_its_staged_file() {
    let scratch = Scratch::new("staging-guard-release");
    let path = reserve_path(scratch.path(), "member").expect("reserve");
    std::fs::write(&path, b"key material").expect("write the staged bytes");

    let released = StagedGuard::new(path.clone()).release();

    assert_eq!(released, path, "release hands the same path on");
    assert!(
        path.exists(),
        "a released path is not removed by the guard that held it"
    );

    let staged = StagedFile::adopt(released, 12, "a".repeat(64));
    drop(staged);
    assert!(!path.exists(), "the staged file removes it exactly once");
}

/// A source that opens but cannot be read: on a Unix host a directory opens for
/// reading and then fails every read, which is the same shape as a source that
/// goes away between the reserve and the copy. The staged file is created
/// before the first read, so this is the path that used to leak it.
#[cfg(unix)]
#[test]
fn a_read_failure_during_the_copy_removes_the_staged_file() {
    let scratch = Scratch::new("staging-read-error");
    let source = scratch.join("source-is-a-directory");
    std::fs::create_dir(&source).expect("a directory source");

    let error = stage_copy(&source, scratch.path(), "member")
        .expect_err("a directory cannot be staged as a member");

    assert_eq!(error.code, clio_types::ErrorCode::Internal);
    assert!(
        error.message.contains("staging"),
        "the message names the step: {}",
        error.message
    );
    assert!(
        error.message.contains(source.to_string_lossy().as_ref()),
        "the message names the source: {}",
        error.message
    );
    let temporaries = staged_temporaries(scratch.path());
    assert!(
        temporaries.is_empty(),
        "the staged file survived the failed copy: {temporaries:?}"
    );
}

/// A sink that fails once its byte budget is spent, and can also refuse the
/// final flush, so a failure lands part-way through a stream instead of at the
/// first write.
struct FailSink {
    remaining: usize,
    fail_flush: bool,
}

impl Write for FailSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > self.remaining {
            return Err(std::io::Error::other("simulated out of space"));
        }
        self.remaining -= buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl StagedSink for FailSink {
    fn sync(&mut self) -> std::io::Result<()> {
        if self.fail_flush {
            Err(std::io::Error::other("simulated flush failure"))
        } else {
            Ok(())
        }
    }
}

/// A source that hands out a fixed number of bytes and remembers how far it
/// got, so a test can see that a failed copy stopped where the write failed.
struct CountedBytes {
    data: Vec<u8>,
    at: usize,
}

impl CountedBytes {
    fn new(len: usize) -> Self {
        Self {
            data: vec![0x5au8; len],
            at: 0,
        }
    }
}

impl Read for CountedBytes {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = (self.data.len() - self.at).min(buf.len());
        buf[..want].copy_from_slice(&self.data[self.at..self.at + want]);
        self.at += want;
        Ok(want)
    }
}

/// The write failing half-way through is what a full disk does: the first chunk
/// lands, the next one cannot, and the error has to name the step.
#[test]
fn a_write_failure_part_way_through_the_copy_names_the_step() {
    let total = COPY_CHUNK * 3;
    let mut input = CountedBytes::new(total);
    let mut sink = FailSink {
        remaining: COPY_CHUNK,
        fail_flush: false,
    };

    let error = copy_and_measure(&mut input, &mut sink, "staging the payload")
        .expect_err("the second chunk cannot be written");

    assert_eq!(error.code, clio_types::ErrorCode::Internal);
    assert!(
        error.message.contains("writing a staged member"),
        "{}",
        error.message
    );
    assert!(
        input.at < total,
        "the copy stopped at the failed write, {} of {total} bytes",
        input.at
    );
}

/// A flush that fails after a complete copy is still a failed staging, and it
/// is named like the others.
#[test]
fn a_flush_failure_after_a_complete_copy_names_the_step() {
    let total = COPY_CHUNK;
    let mut input = CountedBytes::new(total);
    let mut sink = FailSink {
        remaining: usize::MAX,
        fail_flush: true,
    };

    let error = copy_and_measure(&mut input, &mut sink, "staging the payload")
        .expect_err("the final flush fails");

    assert!(
        error.message.contains("flushing a staged member"),
        "{}",
        error.message
    );
    assert_eq!(
        input.at, total,
        "every byte was read; only the flush failed"
    );
}

/// A sink whose write fails the way a full volume's does, so the staging loop's
/// own classification can be checked without a full volume.
struct FullSink;

impl Write for FullSink {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(std::io::ErrorKind::StorageFull))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl StagedSink for FullSink {
    fn sync(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A full volume is the machine's condition, so it is reported as out of space and
/// not as a fault in this code. The kind is used rather than a raw errno here,
/// because this asserts the classification the standard library gives that
/// condition, which is the same on every host.
#[test]
fn a_full_filesystem_is_reported_as_out_of_space() {
    let error = io_error(
        "writing a staged member",
        &std::io::Error::from(std::io::ErrorKind::StorageFull),
    );

    assert_eq!(
        error.code,
        clio_types::ErrorCode::QuotaExceeded,
        "a full filesystem is not this code's fault: {}",
        error.message
    );
}

/// The same condition as a real kernel reports it: ENOSPC is 28 on Linux and on
/// macOS, so this is the errno an actually full volume returns.
#[test]
fn the_real_out_of_space_errno_is_reported_as_out_of_space() {
    let raw = std::io::Error::from_raw_os_error(28);

    assert_eq!(
        raw.kind(),
        std::io::ErrorKind::StorageFull,
        "errno 28 is ENOSPC on this host: {raw:?}"
    );
    let error = io_error("creating a staged member", &raw);

    assert_eq!(
        error.code,
        clio_types::ErrorCode::QuotaExceeded,
        "{}",
        error.message
    );
}

/// A permission failure is deliberately not called out of space: a read-only
/// directory is a different problem with a different fix, and mislabelling it
/// would send the operator to the wrong place.
#[test]
fn a_permission_failure_stays_an_internal_fault() {
    let error = io_error(
        "creating a staged member",
        &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    );

    assert_eq!(
        error.code,
        clio_types::ErrorCode::Internal,
        "{}",
        error.message
    );
}

/// The staging loop itself, not just the wrapper: a write that runs out of space
/// part-way through the copy surfaces as out of space, which is the code the
/// restore path turns into "out of space" for the operator.
#[test]
fn a_write_that_runs_out_of_space_is_reported_as_out_of_space() {
    let mut input = CountedBytes::new(COPY_CHUNK);
    let mut sink = FullSink;

    let error = copy_and_measure(&mut input, &mut sink, "staging the payload")
        .expect_err("the write cannot land on a full volume");

    assert_eq!(
        error.code,
        clio_types::ErrorCode::QuotaExceeded,
        "{}",
        error.message
    );
    assert!(
        error.message.contains("writing a staged member"),
        "the step is named: {}",
        error.message
    );
}
