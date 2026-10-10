// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the shared UTC day conversion in
//! both directions: agreement with an independent calendar reference, calendar
//! validity on every tested day, the named month-end, leap-day, and pre-epoch
//! cases, and the duplication guard that fails if the arithmetic is copied
//! elsewhere.
//!
//! ## Owns
//! - The independent reference walker and the regression assertions
//! - The workspace source scan that pins the arithmetic to `civil.rs`
//! - The refusal of every path that scan could not read or list
//!
//! ## Does not own
//! - The conversion itself (see `civil`)
//! - Timestamp formatting tests in the caller crates
//!
//! ## Boundary
//!
//! Changes to the conversion's arithmetic should normally require changes here
//! only as new assertions. Changes to callers should not require changes here.
//!
//! Keep this module focused on proving and guarding the conversion.

use super::{civil_from_days, days_from_civil};
use crate::civil_test_support::{ReferenceWalker, ref_days_in_month};
use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file under `crates/` and the helper they must not duplicate. The
/// refusals are asserted empty here, so no caller can report a half-scanned
/// workspace as clean by leaving the check out.
fn every_crate_source_file() -> (Vec<ReadFile>, PathBuf) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates_dir = manifest.parent().expect("the crates dir");
    let (read, refusals) = rs_files_under(crates_dir);
    assert!(read.len() > 100, "the scan found {} .rs files", read.len());
    assert_no_refusals(&refusals);
    let helper = crates_dir.join("clio-types").join("src").join("civil.rs");
    (read, helper)
}

/// One path the scan refused, what it refused to do, and the host's reason. A refusal
/// is returned rather than dropped: a file the scan never read never counts as an empty one.
type Refusal = (PathBuf, &'static str, String);

/// A source file the scan read: its path and its text.
type ReadFile = (PathBuf, String);

/// A directory the scan could not list.
const UNLISTABLE_DIR: &str = "a directory the scan could not list";

/// An entry the scan could not read. Its error names no path, so the parent is recorded.
const UNREADABLE_ENTRY: &str = "a directory entry the scan could not read";

/// A file the scan could not open or read to the end.
const UNREADABLE_FILE: &str = "a file the scan could not read";

/// Assert the scan refused nothing, naming every path it could not read or list.
fn assert_no_refusals(refusals: &[Refusal]) {
    assert!(
        refusals.is_empty(),
        "the scan must read every path it walks, but refused {} of them: {refusals:?}",
        refusals.len()
    );
}

/// The scan's only read site: it reads a source file, or records the refusal.
fn read_source(path: &Path, refusals: &mut Vec<Refusal>) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) => {
            refusals.push((path.to_path_buf(), UNREADABLE_FILE, err.to_string()));
            None
        }
    }
}

/// Every day count the issue named converts to its real date, so a regression
/// is traceable to the report.
#[test]
fn civil_from_days_returns_the_days_issue_49_named() {
    assert_eq!(civil_from_days(0), (1970, 1, 1), "the epoch itself");
    assert_eq!(civil_from_days(30), (1970, 1, 31), "the first month end");
    assert_eq!(civil_from_days(19_782), (2024, 2, 29), "the leap day");
}

/// The comparison against the independent reference over the full span. A
/// divergence names the day count and both values.
#[test]
fn civil_from_days_matches_an_independent_reference_for_30000_days() {
    let mut walker = ReferenceWalker::new_at_epoch();
    for days in 0..=30_000i64 {
        assert_eq!(
            civil_from_days(days),
            walker.value(),
            "divergence from the reference at days={days}"
        );
        walker.next();
    }
}

/// The separate validity assertion: month in 1..=12 and day in
/// `1..=days_in_month` on every tested day. A reference comparison and a
/// round-trip can both agree with a `day 0` emission; only this check rejects
/// a date that does not exist.
#[test]
fn civil_from_days_never_emits_an_impossible_date() {
    for days in 0..=30_000i64 {
        let (y, m, d) = civil_from_days(days);
        let last = ref_days_in_month(y, m);
        assert!((1..=12).contains(&m), "days={days} emitted month {m}");
        assert!(
            (1..=last).contains(&d),
            "days={days} emitted {y}-{m:02}-{d:02}, not a real date"
        );
    }
    for days in -1_000..=0i64 {
        let (y, m, d) = civil_from_days(days);
        let last = ref_days_in_month(y, m);
        assert!((1..=12).contains(&m), "days={days} emitted month {m}");
        assert!(
            (1..=last).contains(&d),
            "days={days} emitted {y}-{m:02}-{d:02}, not a real date"
        );
    }
}

/// Negative day counts convert to real pre-1970 dates; the era handling is
/// confirmed against the reference rather than assumed.
#[test]
fn civil_from_days_handles_days_before_the_epoch() {
    assert_eq!(civil_from_days(-1), (1969, 12, 31));
    assert_eq!(civil_from_days(-365), (1969, 1, 1));
    assert_eq!(civil_from_days(-366), (1968, 12, 31));
    let mut walker = ReferenceWalker::new_at_epoch();
    for days in 1..=1_000i64 {
        walker.prev();
        let day_count = -days;
        assert_eq!(
            civil_from_days(day_count),
            walker.value(),
            "divergence from the reference at days={day_count}"
        );
    }
}

/// Every month end of a leap year, each as a named case: the days the defect
/// clustered on.
#[test]
fn civil_from_days_returns_every_month_end_of_a_leap_year() {
    let month_ends: [(i64, i64, u32, u32); 12] = [
        (19_753, 2024, 1, 31),
        (19_782, 2024, 2, 29),
        (19_813, 2024, 3, 31),
        (19_843, 2024, 4, 30),
        (19_874, 2024, 5, 31),
        (19_904, 2024, 6, 30),
        (19_935, 2024, 7, 31),
        (19_966, 2024, 8, 31),
        (19_996, 2024, 9, 30),
        (20_027, 2024, 10, 31),
        (20_057, 2024, 11, 30),
        (20_088, 2024, 12, 31),
    ];
    for (days, y, m, d) in month_ends {
        assert_eq!(civil_from_days(days), (y, m, d), "month end at days={days}");
    }
}

/// The leap day of a leap year and the last day of a common-year February.
#[test]
fn civil_from_days_returns_the_leap_day_and_common_year_feb_28() {
    assert_eq!(civil_from_days(19_782), (2024, 2, 29), "2024-02-29");
    assert_eq!(civil_from_days(19_416), (2023, 2, 28), "2023-02-28");
}

/// The century boundary: 2000-01-01 and the century's own leap day.
#[test]
fn civil_from_days_crosses_the_2000_century_boundary() {
    assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    assert_eq!(civil_from_days(11_016), (2000, 2, 29));
}

/// The inverse direction against the same independent reference: the walker
/// produces the date, this module's arithmetic turns it back into the day
/// count the walker started from. A divergence names the day count.
#[test]
fn days_from_civil_matches_an_independent_reference_for_30000_days() {
    let mut walker = ReferenceWalker::new_at_epoch();
    for days in 0..=30_000i64 {
        let (y, m, d) = walker.value();
        assert_eq!(
            days_from_civil(y, m, d),
            days,
            "divergence from the reference at {y}-{m:02}-{d:02}"
        );
        let (ry, rm, rd) = civil_from_days(days);
        assert_eq!(
            days_from_civil(ry, rm, rd),
            days,
            "round trip at days={days}"
        );
        walker.next();
    }
}

/// The pre-epoch direction, where the era term goes negative: the same era
/// handling the forward direction is checked against the reference.
#[test]
fn days_from_civil_handles_dates_before_the_epoch() {
    let mut walker = ReferenceWalker::new_at_epoch();
    for day_count in 1..=1_000i64 {
        walker.prev();
        let (y, m, d) = walker.value();
        assert_eq!(
            days_from_civil(y, m, d),
            -day_count,
            "divergence from the reference at {y}-{m:02}-{d:02}"
        );
    }
    assert_eq!(days_from_civil(1969, 12, 31), -1);
    assert_eq!(days_from_civil(1969, 1, 1), -365);
    assert_eq!(days_from_civil(1968, 12, 31), -366);
}

/// The named day counts the forward direction pins, read back the other way,
/// plus every month end of a leap year. These are the cases a March-shifted
/// year or a wrong month branch would get wrong.
#[test]
fn days_from_civil_returns_the_named_day_counts() {
    assert_eq!(days_from_civil(1970, 1, 1), 0, "the epoch itself");
    assert_eq!(days_from_civil(1970, 1, 31), 30, "the first month end");
    assert_eq!(days_from_civil(2024, 2, 29), 19_782, "the leap day");
    assert_eq!(days_from_civil(2000, 1, 1), 10_957, "the century boundary");
    assert_eq!(days_from_civil(2000, 2, 29), 11_016, "the century leap day");
    let month_ends: [(i64, u32, u32, i64); 12] = [
        (2024, 1, 31, 19_753),
        (2024, 2, 29, 19_782),
        (2024, 3, 31, 19_813),
        (2024, 4, 30, 19_843),
        (2024, 5, 31, 19_874),
        (2024, 6, 30, 19_904),
        (2024, 7, 31, 19_935),
        (2024, 8, 31, 19_966),
        (2024, 9, 30, 19_996),
        (2024, 10, 31, 20_027),
        (2024, 11, 30, 20_057),
        (2024, 12, 31, 20_088),
    ];
    for (y, m, d, days) in month_ends {
        assert_eq!(days_from_civil(y, m, d), days, "{y}-{m:02}-{d:02}");
    }
}

/// Every calendar day over a three-year span is one day after the day before
/// it, across every month boundary including the leap February. A wrong month
/// branch shows up here as a jump or a stall.
#[test]
fn days_from_civil_advances_by_one_for_every_calendar_day() {
    let mut walker = ReferenceWalker::new_at_epoch();
    let mut previous = days_from_civil(1970, 1, 1);
    for _ in 0..1_095 {
        walker.next();
        let (y, m, d) = walker.value();
        let current = days_from_civil(y, m, d);
        assert_eq!(
            current,
            previous + 1,
            "{y}-{m:02}-{d:02} is not the next day"
        );
        previous = current;
    }
}

/// Every `.rs` file under `root`, read, plus every path the walk refused to read.
/// The same walk is in `clio-config`'s token-comparison scan. Each crate keeps its
/// own copy rather than share public API, so a change to one goes in the other.
fn rs_files_under(root: &Path) -> (Vec<ReadFile>, Vec<Refusal>) {
    let mut read = Vec::new();
    let mut refusals = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                refusals.push((dir.clone(), UNLISTABLE_DIR, err.to_string()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    // Defensive: provoking this needs a directory mutated between
                    // the listing and this read, so no test proves it fires.
                    refusals.push((dir.clone(), UNREADABLE_ENTRY, err.to_string()));
                    continue;
                }
            };
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && let Some(text) = read_source(&path, &mut refusals)
            {
                read.push((path, text));
            }
        }
    }
    (read, refusals)
}

/// The arithmetic is pinned to the shared helper: any copy of it anywhere else
/// in the workspace fails this guard. The needles are built at run time so
/// this test file never contains the raw constants it searches for.
///
/// The scan matches the constants with and without the digit separator Rust
/// accepts, so a copy reformatted as one unbroken literal is caught too. The
/// failure message names the offending file, because a bare "the arithmetic
/// must live only in the shared helper" would be read as duplication when a doc
/// comment merely mentioned the number.
#[test]
fn the_epoch_day_arithmetic_lives_only_in_the_shared_helper() {
    let epoch_shift = format!("{}_{}", 719, "468");
    let era_days = format!("{}_{}", 146, "097");
    let needles = [
        epoch_shift.clone(),
        epoch_shift.replace('_', ""),
        era_days.clone(),
        era_days.replace('_', ""),
    ];
    let (read, helper) = every_crate_source_file();

    let mut offenders = Vec::new();
    for (file, text) in &read {
        if needles.iter().any(|needle| text.contains(needle.as_str())) {
            offenders.push(file.clone());
        }
    }
    assert_eq!(
        offenders,
        vec![helper.clone()],
        "the epoch-day arithmetic must live only in {helper:?}; a file above \
         also spells the era constant, so either the arithmetic was copied or \
         a comment mentions the number"
    );
}

/// The structural companion to the constant scan: a second implementation that
/// does not spell the era constants — a reformatted copy, or a different
/// calendar method such as the workspace's `chrono` paths — would pass the
/// scan above. A day-count conversion is identifiable by its signature, so the
/// guard keys on the return type the callers destructure: no crate other than
/// `clio-types` may define a function whose return type is exactly the
/// conversion's `(i64, u32, u32)`.
///
/// A wider tuple is not a match: `clio-backup`'s `parts` legitimately returns
/// the same triple plus three more components.
#[test]
fn no_other_crate_defines_a_day_count_to_civil_date_function() {
    let (read, _) = every_crate_source_file();
    let signature = "(i64, u32, u32)";

    let mut offenders = Vec::new();
    for (file, text) in &read {
        if file.components().all(|c| c.as_os_str() != "clio-types") && text.contains(signature) {
            offenders.push(file.clone());
        }
    }
    assert_eq!(
        offenders,
        Vec::<PathBuf>::new(),
        "no crate outside clio-types may declare a function returning \
         {signature}; a day-count conversion must be added to the shared \
         helper instead of reimplemented, and an unrelated function using that \
         tuple should name its types differently to stay out of this guard's way"
    );
}

/// A directory that is unique per call and removed when the guard drops.
///
/// `tempfile` is not a workspace dependency, so this is the repo's own fixture
/// pattern: a unique name under the OS temp directory, the pid and the nanos
/// keeping two tests and two runs apart.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let name = format!("clio-civil-scan-{label}-{}-{unique}", std::process::id());
        let path = std::env::temp_dir().join(name);
        fs::create_dir_all(&path).expect("the temp directory can be created");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A source file the scan cannot read is a refusal, not an absence. A dangling
/// symlink stands in for it: the walk reaches it, and opening it fails, so it
/// must leave the results and leave a refusal behind. This one calls
/// `rs_files_under` directly, because here the refusal is the answer.
#[test]
#[cfg(unix)]
fn the_scan_refuses_a_dangling_source_file_instead_of_dropping_it() {
    let dir = TempDir::new("dangling");
    let link = dir.0.join("dangling.rs");
    std::os::unix::fs::symlink(dir.0.join("gone.rs"), &link)
        .expect("the link to a missing file is created");

    let (read, refusals) = rs_files_under(&dir.0);

    assert!(
        !read.iter().any(|(path, _)| path == &link),
        "a file the scan could not read cannot join the read results: {read:?}"
    );
    assert_eq!(refusals.len(), 1, "one unreadable file, found {refusals:?}");
    let (path, what, _) = &refusals[0];
    assert_eq!(path, &link, "the refusal names the file it could not read");
    assert_eq!(*what, UNREADABLE_FILE, "a file, not a directory, refused");
}

/// A root that is a regular file cannot be listed, so the scan must record the
/// root and return no files. No files alone would read as an empty directory. It
/// calls `rs_files_under` directly, because here the refusal is the answer.
#[test]
fn the_scan_refuses_a_root_that_is_a_file_rather_than_a_directory() {
    let dir = TempDir::new("root-is-a-file");
    let file = dir.0.join("not-a-directory.rs");
    fs::write(&file, "// not a directory\n").expect("the fixture file is written");

    let (files, refusals) = rs_files_under(&file);

    assert!(files.is_empty(), "no files come out of a file root");
    assert_eq!(refusals.len(), 1, "one unlistable root, found {refusals:?}");
    let (path, what, _) = &refusals[0];
    assert_eq!(path, &file, "the refusal names the root it could not list");
    assert_eq!(*what, UNLISTABLE_DIR, "a directory refused, not a file");
}
