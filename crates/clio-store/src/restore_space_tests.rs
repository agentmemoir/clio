// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the restore pre-flight: the POSIX
//! `df` report parser, the grouping of the footprints that share a directory,
//! the refusal that names what does not fit, and the production check against a
//! real directory, a path that is not a directory, and a directory that refuses
//! writes.
//!
//! ## Owns
//! - The fixed-capacity check double and the pre-flight grouping assertions
//! - The `df` report parser and the failed-check arms that need no real `df`
//!
//! ## Does not own
//! - How `df` itself is found (see `restore_space_df_tests`)
//! - The swap that runs after the pre-flight (see `restore_swap_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on `restore_space` behavior.

use super::*;

/// A check with a fixed capacity, so an insufficient-space refusal is
/// exercised without filling a real filesystem.
struct FixedCheck {
    capacity: u64,
}

impl SpaceCheck for FixedCheck {
    fn can_hold(&self, dir: &Path, bytes: u64) -> Result<(), AmError> {
        if bytes <= self.capacity {
            return Ok(());
        }
        Err(AmError::new(
            ErrorCode::QuotaExceeded,
            format!(
                "{} bytes are available in {}, but {bytes} bytes are needed",
                self.capacity,
                dir.display()
            ),
        ))
    }
}

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-restore-space-{tag}"
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether this host has a `df` at one of the fixed absolute paths.
///
/// A host that does not is a real layout, not a broken one, so an assertion
/// that needs a live `df` is skipped rather than failed. The behaviour that arm
/// would have covered is covered host-free by
/// `no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried` in the
/// sibling `df` test module.
fn host_has_df() -> bool {
    resolve_df().is_some()
}

#[test]
fn a_linux_df_report_parses() {
    let report = "Filesystem     1024-blocks     Used Available Capacity Mounted on\n\
                  /dev/sda1        100000000 50000000  50000000      50% /\n";
    assert_eq!(parse_available_kb(report), Some(50_000_000));
}

#[test]
fn a_macos_df_report_parses() {
    let report = "Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n\
                  /dev/disk3s1s1   482797652  21613272 429861636     5%  500000    0   100%   /\n";
    assert_eq!(parse_available_kb(report), Some(429_861_636));
}

#[test]
fn a_report_without_a_readable_column_is_none() {
    assert_eq!(parse_available_kb(""), None);
    assert_eq!(
        parse_available_kb("Filesystem 1024-blocks Used Available\n"),
        None
    );
    assert_eq!(
        parse_available_kb("Filesystem 1024-blocks Used Available\n/x 1 2 not-a-number\n"),
        None
    );
}

#[test]
fn the_refusal_names_the_footprint_that_does_not_fit() {
    let scratch = Scratch::new("footprint");
    let check = FixedCheck { capacity: 10 };
    let footprints = [
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 10,
            label: "the staged payload",
        },
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 11,
            label: "the pre-restore snapshot",
        },
    ];
    // Both footprints land in one directory, so the directory is asked for their
    // sum of 21 against a ceiling of 10. The refusal must still name the second
    // footprint, which is the one that pushed the total over.
    let error =
        preflight_restore(&footprints, &check).expect_err("the second footprint is too big");
    assert_eq!(error.code, ErrorCode::QuotaExceeded);
    assert!(
        error.message.contains("the pre-restore snapshot"),
        "the message names the footprint: {}",
        error.message
    );
    assert!(
        error.message.contains(&scratch.0.display().to_string()),
        "the message names the directory: {}",
        error.message
    );
}

#[test]
fn a_single_footprint_refusal_names_only_that_footprint() {
    // A directory with one footprint must produce exactly the message it always
    // did: its own label, then the directory, then the check's own words. No
    // "and", no second label, nothing else added by the grouping.
    let scratch = Scratch::new("one-label");
    let check = FixedCheck { capacity: 10 };
    let footprints = [SpaceFootprint {
        dir: scratch.0.clone(),
        bytes: 11,
        label: "the pre-restore snapshot",
    }];
    let error = preflight_restore(&footprints, &check).expect_err("11 bytes is too many");
    assert_eq!(error.code, ErrorCode::QuotaExceeded);
    assert_eq!(
        error.message,
        format!(
            "the pre-restore snapshot cannot be placed in {}: 10 bytes are available in {}, but 11 \
             bytes are needed",
            scratch.0.display(),
            scratch.0.display()
        ),
        "one footprint produces the one-label message, unchanged"
    );
}

#[test]
fn footprints_that_share_a_directory_are_named_together_in_one_phrase() {
    // The message names what does not fit, and after grouping that is the whole
    // group, not the last footprint to be checked. Both labels must appear, in
    // the order the footprints were given.
    let scratch = Scratch::new("multi-label");
    let check = FixedCheck { capacity: 10 };
    let footprints = [
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 4,
            label: "the restored payload",
        },
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 4,
            label: "the archive's key member",
        },
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 4,
            label: "the pre-restore snapshot",
        },
    ];
    let error = preflight_restore(&footprints, &check)
        .expect_err("12 bytes cannot fit where 10 are available");
    assert_eq!(error.code, ErrorCode::QuotaExceeded);
    assert!(
        error.message.starts_with(&format!(
            "the restored payload, the archive's key member and the pre-restore snapshot cannot \
             be placed in {}: ",
            scratch.0.display()
        )),
        "the message joins every label of the group with commas and a final and: {}",
        error.message
    );
    assert!(
        error.message.contains("10 bytes are available")
            && error.message.contains("12 bytes are needed"),
        "the inner message reports the directory's total, not one footprint: {}",
        error.message
    );
}

#[test]
fn two_footprints_that_each_fit_but_whose_sum_does_not_are_refused() {
    // The case the sum exists for: each half fits under the ceiling on its own,
    // and the pair does not. A per-footprint check would pass both and the
    // second would then fail to stage with no room left.
    let scratch = Scratch::new("sum-refused");
    let check = FixedCheck { capacity: 100 };
    let footprints = [
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 60,
            label: "the restored payload",
        },
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 60,
            label: "the archive's key member",
        },
    ];
    let error = preflight_restore(&footprints, &check)
        .expect_err("60 + 60 cannot fit where 100 are available");
    assert_eq!(error.code, ErrorCode::QuotaExceeded);
    assert!(
        error.message.starts_with(&format!(
            "the restored payload and the archive's key member cannot be placed in {}: ",
            scratch.0.display()
        )),
        "both members of the shared directory are named: {}",
        error.message
    );
    assert!(
        error.message.contains("120 bytes are needed"),
        "the check was asked for the sum, not for one member: {}",
        error.message
    );
}

#[test]
fn a_group_total_too_large_to_add_refuses_instead_of_wrapping() {
    // `u64::MAX + u64::MAX` is `u64::MAX - 1` in wrapping arithmetic, so the
    // ceiling sits one byte below the saturated total: a plain `+` would produce
    // a total this check accepts and this test would fail, while
    // `saturating_add` pins the total at `u64::MAX` and the check refuses.
    let scratch = Scratch::new("saturating");
    let check = FixedCheck {
        capacity: u64::MAX - 1,
    };
    let footprints = [
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: u64::MAX,
            label: "the restored payload",
        },
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: u64::MAX,
            label: "the archive's key member",
        },
    ];
    let error = preflight_restore(&footprints, &check).expect_err(
        "a wrapped total would fit under a ceiling of u64::MAX - 1, so this refusal is what \
         proves the sum saturates instead of wrapping",
    );
    assert_eq!(error.code, ErrorCode::QuotaExceeded);
    assert!(
        error
            .message
            .contains(&format!("{} bytes are needed", u64::MAX)),
        "the total saturated at u64::MAX rather than wrapping down: {}",
        error.message
    );
}

#[test]
fn a_group_with_no_label_yields_no_phrase() {
    // `preflight_restore` never builds an empty group, so this arm exists only
    // to keep the phrase builder total rather than panicking inside a safety
    // gate. It is pinned here so the arm is a stated decision and not a line
    // that quietly stopped running.
    assert_eq!(join_labels(&[]), "");
}

#[test]
fn both_footprints_fitting_passes() {
    let scratch = Scratch::new("fits");
    let check = FixedCheck { capacity: 100 };
    let footprints = [
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 60,
            label: "the staged payload",
        },
        SpaceFootprint {
            dir: scratch.0.clone(),
            bytes: 40,
            label: "the pre-restore snapshot",
        },
    ];
    assert!(preflight_restore(&footprints, &check).is_ok());
}

#[test]
fn the_production_check_passes_for_a_small_footprint() {
    // Host-dependent: the production check needs a live `df` to read free space.
    // On a host with none of the fixed paths, the injected test
    // `no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried` in
    // the sibling `df` test module covers what this host cannot answer.
    if !host_has_df() {
        return;
    }
    let scratch = Scratch::new("disk");
    assert!(DiskSpace.can_hold(&scratch.0, 4096).is_ok());
    assert!(
        DiskSpace.can_hold(&scratch.0, 0).is_ok(),
        "zero still probes"
    );
}

#[test]
fn the_production_check_refuses_more_than_any_filesystem_holds() {
    // Host-dependent, as above: without a `df` the check cannot read free space.
    if !host_has_df() {
        return;
    }
    let scratch = Scratch::new("huge");
    let error = DiskSpace
        .can_hold(&scratch.0, u64::MAX)
        .expect_err("no filesystem holds u64::MAX bytes");
    assert_eq!(error.code, ErrorCode::QuotaExceeded);
}

#[test]
fn the_production_check_refuses_a_path_that_is_not_a_directory() {
    let scratch = Scratch::new("not-dir");
    let file = scratch.0.join("clio.db");
    std::fs::write(&file, b"x").expect("seed the file");
    let error = DiskSpace
        .can_hold(&file, 1)
        .expect_err("a file is not a directory");
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        error.message.contains("is not a directory"),
        "{}",
        error.message
    );
}

/// Whether the mode bits stop this process writing into `dir`.
///
/// Root, and any process holding `CAP_DAC_OVERRIDE`, writes into a directory
/// whatever its mode, so a test that asserts a write refusal cannot observe one
/// there. The probe asks the question directly instead of reading the uid, and
/// it removes the file it created when the write is allowed.
#[cfg(unix)]
fn write_is_refused(dir: &Path) -> bool {
    let probe = dir.join(".write-probe");
    match std::fs::write(&probe, b"x") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            false
        }
        Err(_) => true,
    }
}

/// The refusal is only observable to a process the mode bits restrict, so the
/// test asks the process's own access which case this is: a restricted process
/// gets the refusal, and a process that bypasses the mode bits gets the answer
/// the space it found allows.
#[cfg(unix)]
#[test]
fn the_production_check_refuses_a_directory_that_denies_writes() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("readonly");
    let locked = scratch.0.join("locked");
    std::fs::create_dir_all(&locked).expect("create the locked directory");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500))
        .expect("remove write permission");
    if write_is_refused(&locked) {
        let error = DiskSpace
            .can_hold(&locked, 1)
            .expect_err("a read-only directory must refuse");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("not writable"), "{}", error.message);
    } else {
        DiskSpace
            .can_hold(&locked, 1)
            .expect("a DAC-bypassing process passes the writability probe");
    }
    // Restore the mode so the scratch cleanup can remove the directory.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700))
        .expect("restore write permission");
}

#[test]
fn a_missing_df_is_reported_as_a_failed_check() {
    let scratch = Scratch::new("no-df");
    let error = run_available_bytes(&scratch.0, &|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no df on this host",
        ))
    })
    .expect_err("a missing df cannot answer");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("`df` failed"), "{}", error.message);
}

#[cfg(unix)]
#[test]
fn a_df_that_fails_is_reported_as_a_failed_check() {
    let scratch = Scratch::new("df-fails");
    let error = run_available_bytes(&scratch.0, &|_| {
        std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 3")
            .output()
    })
    .expect_err("a failing df cannot answer");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("`df` exited with"),
        "{}",
        error.message
    );
}

#[test]
fn a_df_report_with_no_readable_column_is_reported_as_a_failed_check() {
    let scratch = Scratch::new("df-junk");
    // A successful command with empty output is the shape a report that cannot
    // be parsed takes.
    let error = run_available_bytes(&scratch.0, &|_| std::process::Command::new("true").output())
        .expect_err("an unreadable report cannot answer");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(
        error.message.contains("available-space"),
        "{}",
        error.message
    );
}
