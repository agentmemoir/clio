// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the holder-lookup unit coverage: the
//! `/proc/locks` line parser against well-formed and malformed lines, the
//! device-number split, and the pid record's parser. The live lookup against
//! a real second process runs in `store_lock_process_tests`.
//!
//! ## Owns
//! - These parser assertions
//!
//! ## Does not own
//! - The lock primitive under test (see `store_lock`)
//!
//! ## Boundary
//!
//! Keep this file focused on parsing what a holder's pid is read from.

#[cfg(target_os = "linux")]
use super::dev_parts;
#[cfg(target_os = "linux")]
use super::parse_locks_line;
use super::recorded_pid;
use std::path::PathBuf;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-lock-pids-{tag}"
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_well_formed_flock_line_parses_to_its_holder() {
    let entry = parse_locks_line("1: FLOCK  ADVISORY  WRITE 162502 00:26:160885 0 EOF")
        .expect("a flock entry");
    assert_eq!(entry.mode, "WRITE");
    assert_eq!(entry.pid, 162_502);
    assert_eq!(entry.dev, (0, 38));
    assert_eq!(entry.inode, 160_885);
}

#[cfg(target_os = "linux")]
#[test]
fn lines_that_are_not_flock_entries_or_lack_fields_yield_none() {
    // A POSIX record lock, not a flock.
    assert!(
        parse_locks_line("2: POSIX  ADVISORY  READ  0 fd:03:525920 0 EOF").is_none(),
        "a POSIX line is not a flock holder"
    );
    // Too few fields to carry the pid and the device.
    assert!(parse_locks_line("3: FLOCK ADVISORY WRITE").is_none());
    // A device field without all three parts.
    assert!(
        parse_locks_line("4: FLOCK ADVISORY WRITE 42 00:26 0 EOF").is_none(),
        "a device without an inode is not parseable"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_field_that_cannot_be_parsed_yields_none() {
    assert!(
        parse_locks_line("5: FLOCK ADVISORY WRITE notapid 00:26:160885 0 EOF").is_none(),
        "a pid that is not a number is not a holder"
    );
    assert!(
        parse_locks_line("6: FLOCK ADVISORY WRITE 42 zz:26:160885 0 EOF").is_none(),
        "a device that is not hex is not a holder"
    );
    assert!(
        parse_locks_line("7: FLOCK ADVISORY WRITE 42 00:26:notanino 0 EOF").is_none(),
        "an inode that is not a number is not a holder"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_stat_device_id_splits_into_major_and_minor() {
    assert_eq!(dev_parts(38), (0, 38), "a minor-only device");
    assert_eq!(dev_parts(64771), (253, 3), "major 0xfd, minor 3");
    assert_eq!(
        dev_parts(2_161_920),
        (253, 512),
        "a minor above 255 keeps its high bits"
    );
    assert_eq!(
        dev_parts(17_592_186_275_841),
        (5000, 1),
        "a major above 4095 keeps its high bits"
    );
}

#[test]
fn a_recorded_pid_line_parses_and_garbage_does_not() {
    let scratch = Scratch::new("pid-parse");
    let lock = scratch.0.join("clio.db.lock");
    std::fs::write(&lock, b"pid 4242\n").expect("write a pid line");
    assert_eq!(recorded_pid(&lock), Some(4242));
    std::fs::write(&lock, b"not a pid\n").expect("write garbage");
    assert_eq!(recorded_pid(&lock), None, "garbage is not a pid");
    assert_eq!(recorded_pid(&scratch.0.join("absent.lock")), None);
}
