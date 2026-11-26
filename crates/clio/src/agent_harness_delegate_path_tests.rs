// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the binary lookup and the held descriptor: what `Held`
//! opens, what it refuses, and that a name replaced after the open is not what
//! the descriptor names.
//!
//! ## Owns
//! - The descriptor identity after a replace, and the refuses and accepts of
//!   `Held::open`
//! - The directory-safety truth table the no-descriptor platform uses
//!
//! ## Does not own
//! - The lookup and hold implementation (`agent_harness_delegate_path`)
//! - Starting a vendor command and judging its output (`agent_harness_delegate`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::agent_harness_delegate::test_support_tests::Fixture;
use std::fs;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
fn write_file(fixture: &Fixture, name: &str, contents: &str, mode: u32) -> PathBuf {
    let path = fixture.root.join(name);
    fs::write(&path, contents).expect("the test file can be written");
    fs::set_permissions(&path, fs::Permissions::from_mode(mode))
        .expect("the test file mode can be set");
    path
}

#[test]
fn lookup_on_path_misses_a_binary_no_host_ships() {
    assert_eq!(
        lookup_on_path("clio-no-such-binary-anywhere"),
        None,
        "a binary absent from every PATH entry is not found"
    );
}

#[cfg(unix)]
#[test]
fn is_runnable_follows_the_exec_bits_of_a_real_file() {
    let fixture = Fixture::new("is-runnable");
    let script = fixture.root.join("runnable");
    fs::write(&script, "#!/bin/sh\n").expect("the script can be written");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
        .expect("the script can be made executable");
    assert!(is_runnable(&script), "an executable file is runnable");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o644))
        .expect("the exec bits can be removed");
    assert!(
        !is_runnable(&script),
        "a file without exec bits is not runnable"
    );
    assert!(
        !is_runnable(&fixture.root.join("missing")),
        "a missing file is not runnable"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn held_program_names_the_opened_file_after_the_path_is_replaced() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    let fixture = Fixture::new("path-replaced");
    let first = write_file(&fixture, "first", "#!/bin/sh\nexit 0\n", 0o755);
    let second = write_file(&fixture, "second", "#!/bin/sh\nexit 0\n", 0o755);
    let first_meta = fs::metadata(&first).expect("the first file can be inspected");
    let second_meta = fs::metadata(&second).expect("the second file can be inspected");

    let held = Held::open(&first).expect("the first file is a runnable regular file");
    fs::rename(&second, &first).expect("the second file replaces the first name");

    let held_meta = held
        .file
        .metadata()
        .expect("the held descriptor can be inspected");
    let path_meta = fs::metadata(&first).expect("the replaced name can be inspected");
    assert_eq!(
        (held_meta.dev(), held_meta.ino()),
        (first_meta.dev(), first_meta.ino())
    );
    assert_eq!(
        (path_meta.dev(), path_meta.ino()),
        (second_meta.dev(), second_meta.ino())
    );
    assert_ne!(
        held_meta.ino(),
        path_meta.ino(),
        "the held file is no longer the file at the path"
    );

    let expected = format!("/proc/self/fd/{}", held.file.as_raw_fd());
    assert_eq!(
        held.command(&first).get_program(),
        std::ffi::OsStr::new(expected.as_str())
    );
}

#[cfg(unix)]
#[test]
fn open_refuses_a_directory() {
    let fixture = Fixture::new("path-directory");
    let err = Held::open(&fixture.root)
        .err()
        .expect("a directory is not a runnable program");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[cfg(unix)]
#[test]
fn open_refuses_a_file_without_an_exec_bit() {
    let fixture = Fixture::new("path-no-exec");
    let path = write_file(&fixture, "plain", "#!/bin/sh\nexit 0\n", 0o644);
    let err = Held::open(&path)
        .err()
        .expect("a file without an exec bit is not runnable");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[cfg(unix)]
#[test]
fn open_accepts_a_symlink_to_an_executable() {
    let fixture = Fixture::new("path-symlink");
    let target = write_file(&fixture, "target", "#!/bin/sh\nexit 0\n", 0o755);
    let link = fixture.root.join("link");
    std::os::unix::fs::symlink(&target, &link).expect("the symlink can be created");
    Held::open(&link).expect("a symlink to a runnable file is accepted");
}

#[cfg(unix)]
#[test]
fn dir_is_safe_truth_table() {
    const EUID: u32 = 1000;
    assert!(dir_is_safe(0, 0o755, EUID), "root-owned 0755 is safe");
    assert!(dir_is_safe(EUID, 0o755, EUID), "euid-owned 0755 is safe");
    assert!(
        !dir_is_safe(EUID + 1, 0o755, EUID),
        "another user's 0755 is unsafe"
    );
    assert!(!dir_is_safe(0, 0o775, EUID), "group-writable is unsafe");
    assert!(dir_is_safe(0, 0o700, EUID), "root-owned 0700 is safe");
}
