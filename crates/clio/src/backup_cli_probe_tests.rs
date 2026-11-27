// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio backup dir` writability probe: that it claims a
//! fresh name, that an entry already sitting at a candidate name is left
//! exactly as it was found, and that a taken name is skipped for the next
//! candidate instead of answering "not writable".
//!
//! ## Owns
//! - Behavior tests for the exclusive claim the probe makes on its own name,
//!   and for the retry over its distinct candidate names
//!
//! ## Does not own
//! - The probe's place in the `backup dir` command (see `backup_cli_tests`)
//! - Directory creation and resolution (see `backup_cli` and `clio-config`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::setup_support_tests::temp_dir;

const PLANTED: &str = "a file this probe must not touch\n";

/// An arbitrary path inside `dir` for a test to hand the probe as a candidate.
///
/// `claim_probe_name` accepts any path, so the name carries no meaning here and
/// the test does not pin the production naming scheme.
fn probe_path(dir: &Path, tag: &str) -> PathBuf {
    dir.join(format!("probe-{tag}"))
}

/// The candidate list a test hands the probe: one arbitrary name per tag.
fn candidates(dir: &Path, tags: &[&str]) -> Vec<PathBuf> {
    tags.iter().map(|tag| probe_path(dir, tag)).collect()
}

#[test]
fn probe_claims_a_fresh_name_and_removes_it() {
    let dir = temp_dir("probe-fresh");
    let probe = probe_path(&dir, "fresh");
    assert!(
        claim_probe_name(std::slice::from_ref(&probe)),
        "a writable directory accepts the probe"
    );
    assert!(!probe.exists(), "the probe leaves no entry behind");
}

#[test]
fn probe_reports_not_writable_and_leaves_an_existing_file_intact() {
    let dir = temp_dir("probe-file");
    let probe = probe_path(&dir, "file");
    std::fs::write(&probe, PLANTED).expect("plant a file at the probe name");
    assert!(
        !claim_probe_name(std::slice::from_ref(&probe)),
        "a taken name is reported as not writable"
    );
    assert_eq!(
        std::fs::read_to_string(&probe).expect("read the planted file"),
        PLANTED,
        "the planted file keeps its contents"
    );
}

#[cfg(unix)]
#[test]
fn probe_reports_not_writable_and_never_follows_a_planted_symlink() {
    use std::os::unix::fs::symlink;
    let dir = temp_dir("probe-link");
    let victim = dir.join("victim.txt");
    std::fs::write(&victim, PLANTED).expect("write the link target");
    let probe = probe_path(&dir, "link");
    symlink(&victim, &probe).expect("plant a link at the probe name");
    assert!(
        !claim_probe_name(std::slice::from_ref(&probe)),
        "a name held by a symlink is reported as not writable"
    );
    assert_eq!(
        std::fs::read_to_string(&victim).expect("read the link target"),
        PLANTED,
        "the file behind the link keeps its contents"
    );
    assert!(
        std::fs::symlink_metadata(&probe).is_ok(),
        "the planted link is left in place"
    );
}

#[test]
fn every_taken_candidate_name_reports_not_writable_and_keeps_every_planted_file() {
    let dir = temp_dir("probe-all-taken");
    let all = candidates(&dir, &["first", "second", "third"]);
    for path in &all {
        std::fs::write(path, PLANTED).expect("plant a file at a candidate name");
    }
    assert!(
        !claim_probe_name(&all),
        "a directory whose every candidate name is taken is reported as not writable"
    );
    for path in &all {
        assert_eq!(
            std::fs::read_to_string(path).expect("read a planted file"),
            PLANTED,
            "each planted file keeps the bytes this test wrote"
        );
    }
}

#[test]
fn a_taken_first_candidate_is_skipped_and_a_later_one_is_claimed() {
    let dir = temp_dir("probe-taken-first");
    let all = candidates(&dir, &["taken", "free-one", "free-two"]);
    let taken = &all[0];
    std::fs::write(taken, PLANTED).expect("plant a file at the first candidate name");
    assert!(
        claim_probe_name(&all),
        "a free later candidate name makes a writable directory report writable"
    );
    assert_eq!(
        std::fs::read_to_string(taken).expect("read the planted file"),
        PLANTED,
        "the planted file keeps the bytes this test wrote"
    );
    for path in &all[1..] {
        assert!(
            !path.exists(),
            "the probe removes the one entry it created and leaves no other"
        );
    }
}
