// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the entry identity the confirming stat
//! compares, on a real directory held open as the run holds it.
//!
//! ## Owns
//! - The `same_entry` assertions: one entry compared with itself, two different
//!   files, and a hard link to one inode
//!
//! ## Does not own
//! - The removal rules themselves (see `prune_delete`)
//! - The containment cases (see `prune_delete::containment_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::fixtures::Scratch;
use rustix::fs::{AtFlags, statat};

/// The three names the fixture writes into one held directory.
const FIRST: &str = "first.cliobak.tar.gz";
const SECOND: &str = "second.cliobak.tar.gz";
const ALIAS: &str = "alias.cliobak.tar.gz";

/// Device and inode together are one entry's identity.
///
/// The same file stat'd twice through the held descriptor compares equal, so a
/// run that inspected and confirmed one name removes it. Two different files in
/// one directory compare unequal, which is what turns a swap in the gap into a
/// refusal instead of a silent removal of something never inspected. A hard link
/// to the same inode under a second name compares equal, because a second name
/// for one file is still that one file.
#[test]
fn the_identity_confirm_matches_one_entry_and_separates_two_files() {
    let scratch = Scratch::new("prune-identity");
    std::fs::write(scratch.join(FIRST), b"one").expect("write the first file");
    std::fs::write(scratch.join(SECOND), b"a different length").expect("write the second");
    std::fs::hard_link(scratch.join(FIRST), scratch.join(ALIAS))
        .expect("a hard link to the first file");
    let held = hold(scratch.path()).expect("hold the directory open");
    let stat = |name: &str| {
        statat(&held.dir, name, AtFlags::SYMLINK_NOFOLLOW)
            .expect("stat a bare name in the held dir")
    };

    let first = stat(FIRST);
    let again = stat(FIRST);
    let second = stat(SECOND);
    let alias = stat(ALIAS);

    assert!(
        same_entry(&first, &again),
        "the same file stat'd twice is one entry"
    );
    assert!(
        !same_entry(&first, &second),
        "two different files are two entries"
    );
    assert!(
        same_entry(&first, &alias),
        "a hard link names the same inode"
    );
    assert_eq!(
        first.st_size, alias.st_size,
        "the size credited comes from the inspection"
    );
    assert_ne!(
        first.st_size, second.st_size,
        "the fixture files differ in size on purpose"
    );
}
