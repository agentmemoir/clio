// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the mode Clio leaves on the directory
//! that holds the encryption key file: owner-only after a write into a directory
//! it had to create, and owner-only after a write into one that already existed
//! with a looser mode.
//!
//! ## Owns
//! - The directory mode after `write_key_file` creates a missing parent
//! - The directory mode after `write_key_file` writes into an existing parent
//!
//! ## Does not own
//! - The gate that refuses a link or a foreign owner at a key path (see
//!   `sync_key_gate_tests`)
//! - Where the key file lives and whether the keystore may be read (see the
//!   tests declared by `secret_key_file`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every path here comes from a
//! unique temp directory removed when the guard drops, and no test here reaches
//! the machine's real key material.
//!
//! Each case asserts the mode the code sets, not what the host umask happened to
//! produce, so the result is the same on every machine.
//!
//! Keep this module focused on the directory mode the key file's write leaves.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::secret_key::KEY_LEN;
use crate::secret_key_file::write_key_file;
use crate::sync_key_test_support::TempDir;

/// The permission bits of `path`, without the file-type bits.
fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("the path exists")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn writing_into_a_missing_directory_leaves_it_owner_only() {
    let root = TempDir::new("secret-key-missing-dir");
    let key_dir = root.join("keystore");
    assert!(!key_dir.exists(), "the directory starts missing");
    let key_path = key_dir.join("clio.key");

    write_key_file(&key_path, &[7_u8; KEY_LEN]).expect("the key file is written");

    assert_eq!(mode_of(&key_dir), 0o700, "the created directory is 0700");
    assert_eq!(mode_of(&key_path), 0o600, "the key file is 0600");
    assert_eq!(
        fs::read(&key_path).expect("the key file is read"),
        vec![7_u8; KEY_LEN],
    );
}

#[test]
fn writing_into_a_loose_existing_directory_tightens_it() {
    let root = TempDir::new("secret-key-loose-dir");
    let key_dir = root.join("keystore");
    fs::create_dir_all(&key_dir).expect("the directory is created");
    fs::set_permissions(&key_dir, fs::Permissions::from_mode(0o755))
        .expect("the directory mode is set");
    assert_eq!(mode_of(&key_dir), 0o755, "the directory starts loose");

    write_key_file(&key_dir.join("clio.key"), &[9_u8; KEY_LEN])
        .expect("the key file is written");

    assert_eq!(mode_of(&key_dir), 0o700, "the existing directory is 0700");
}
