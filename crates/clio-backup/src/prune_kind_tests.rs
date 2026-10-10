// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing how a backup listing's symbolic links
//! are told apart from its real files, which is the answer prune selection is
//! given so that a link never counts as a snapshot.
//!
//! ## Owns
//! - The `LinkedPaths::among` and `LinkedPaths::contains` assertions
//! - Real files, links to real archives, dangling links, and unreadable entries
//!
//! ## Does not own
//! - What a prune keeps or deletes (see `prune_tests` and `prune_delete_tests`)
//! - The classification itself (see `prune_kind`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::fixtures::Scratch;
use crate::inventory::SnapshotRow;
use std::path::PathBuf;

/// One listed row, so classification is tested over paths rather than manifests.
fn row(path: PathBuf) -> SnapshotRow {
    SnapshotRow {
        id: crate::archive_name::snapshot_id_of(&path).unwrap_or_default(),
        path,
        created_at: "2026-05-05T00:00:00Z".to_owned(),
        backend: "sqlite".to_owned(),
        slug: "clio".to_owned(),
        archive_bytes: 0,
        payload_bytes: 0,
        keys_included: false,
        label: None,
        unreadable: None,
    }
}

#[test]
fn a_real_file_is_not_a_link_and_a_default_answer_holds_nothing() {
    let scratch = Scratch::new("prune-kind-file");
    let archive = scratch.join("clio-2026-05-05T000000Z-plain.cliobak.tar.gz");
    std::fs::write(&archive, b"not a link").expect("write a real file");

    let links = LinkedPaths::among(&[row(archive.clone())]);

    assert!(!links.contains(&archive), "a real file is not a link");
    assert!(
        !LinkedPaths::default().contains(&archive),
        "the empty answer holds nothing"
    );
}

#[test]
fn an_entry_that_is_not_there_is_not_a_link() {
    let scratch = Scratch::new("prune-kind-missing");
    let missing = scratch.join("clio-2026-05-05T000000Z-gone.cliobak.tar.gz");

    let links = LinkedPaths::among(&[row(missing.clone())]);

    assert!(
        !links.contains(&missing),
        "an entry that cannot be examined is left to the delete-time check"
    );
}

/// Unix-gated because it names links, and only Unix has them.
#[cfg(unix)]
#[test]
fn a_link_to_a_real_archive_is_a_link_however_it_ends() {
    let scratch = Scratch::new("prune-kind-links");
    let target = scratch.join("elsewhere.cliobak.tar.gz");
    std::fs::write(&target, b"an archive elsewhere").expect("write the target");
    let to_real = scratch.join("clio-2026-05-05T000000Z-real.cliobak.tar.gz");
    std::os::unix::fs::symlink(&target, &to_real).expect("link to a target that exists");
    let dangling = scratch.join("clio-2026-05-05T000000Z-dangling.cliobak.tar.gz");
    std::os::unix::fs::symlink(scratch.join("absent"), &dangling).expect("link to nothing");
    let plain = scratch.join("clio-2026-05-05T000000Z-plain.cliobak.tar.gz");
    std::fs::write(&plain, b"a real archive").expect("write a real archive");

    let links = LinkedPaths::among(&[
        row(to_real.clone()),
        row(dangling.clone()),
        row(plain.clone()),
    ]);

    assert!(
        links.contains(&to_real),
        "a link to a real archive is a link"
    );
    assert!(links.contains(&dangling), "a link to nothing is a link too");
    assert!(
        !links.contains(&plain),
        "the real archive beside them is not"
    );
    assert!(!links.contains(&target), "the target is not in the listing");
}
