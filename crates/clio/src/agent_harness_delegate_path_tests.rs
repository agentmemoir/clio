// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `PATH` candidate list and the held binary: what
//! `Held` opens, what it refuses, and that the descriptor starts the file that
//! was opened after its name is replaced.
//!
//! ## Owns
//! - The candidate list's order and join, and the descriptor identity after a
//!   replace
//! - The accepts and refuses of `Held::open`
//! - The directory- and file-safety truth tables the no-descriptor platform
//!   uses
//!
//! ## Does not own
//! - The candidate list and hold implementation
//!   (`agent_harness_delegate_path`)
//! - Starting a vendor command and judging its output (`agent_harness_delegate`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::agent_harness_delegate::test_support_tests::Fixture;
#[cfg(target_os = "linux")]
use crate::agent_harness_delegate::test_support_tests::exec_dir;
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
fn path_candidates_keep_the_path_order() {
    let value = std::ffi::OsString::from("/first/bin:/second/bin");
    assert_eq!(
        path_candidates_from(&value, "claude"),
        vec![
            PathBuf::from("/first/bin/claude"),
            PathBuf::from("/second/bin/claude"),
        ],
        "the candidates keep the order of the PATH entries"
    );
}

#[test]
fn path_candidates_join_the_binary_name_to_each_directory() {
    let value = std::ffi::OsString::from("/opt/homebrew/bin/");
    assert_eq!(
        path_candidates_from(&value, "claude"),
        vec![PathBuf::from("/opt/homebrew/bin/claude")],
        "a trailing slash does not change the joined candidate"
    );
}

#[test]
fn path_candidates_of_an_empty_path_name_the_binary_itself() {
    let value = std::ffi::OsString::new();
    assert_eq!(
        path_candidates_from(&value, "claude"),
        vec![PathBuf::from("claude")],
        "an empty PATH entry names the binary in the current directory"
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
        held.command().get_program(),
        std::ffi::OsStr::new(expected.as_str())
    );
    assert_eq!(held.path(), first, "the stored path stays the candidate");
}

#[cfg(target_os = "linux")]
#[test]
fn held_command_runs_the_opened_file_after_the_path_is_replaced() {
    let dir = exec_dir("held-after-replace").expect("the stub needs a directory that permits exec");
    let first = dir.join("first");
    let second = dir.join("second");
    fs::write(&first, "#!/bin/sh\nprintf 'A\\n'\n").expect("the first stub can be written");
    fs::write(&second, "#!/bin/sh\nprintf 'B\\n'\n").expect("the second stub can be written");
    fs::set_permissions(&first, fs::Permissions::from_mode(0o755))
        .expect("the first stub can be made executable");
    fs::set_permissions(&second, fs::Permissions::from_mode(0o755))
        .expect("the second stub can be made executable");

    let held = Held::open(&first).expect("the first stub is a runnable regular file");
    fs::rename(&second, &first).expect("the second stub replaces the first name");

    let out = held.command().output().expect("the held stub can start");
    assert!(out.status.success(), "the held stub exits cleanly: {out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "A\n",
        "the held file runs, not the file now at the candidate name"
    );
    fs::remove_dir_all(&dir).expect("the stub directory is removed");
}

#[cfg(target_os = "linux")]
#[test]
fn a_script_started_through_the_descriptor_sees_the_descriptor_as_zero() {
    let dir = exec_dir("script-zero").expect("the stub needs a directory that permits exec");
    let fixture = Fixture::new("script-zero");
    let record = fixture.root.join("zero.txt");
    let script = format!("#!/bin/sh\nprintf '%s' \"$0\" > '{}'\n", record.display());
    let stub = dir.join("stub");
    fs::write(&stub, script).expect("the stub script can be written");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755))
        .expect("the stub script can be made executable");

    let held = Held::open(&stub).expect("the stub is a runnable regular file");
    let out = held.command().output().expect("the held script can start");
    assert!(out.status.success(), "the script exits cleanly: {out:?}");

    let zero = fs::read_to_string(&record).expect("the script recorded $0");
    assert!(
        zero.starts_with("/proc/self/fd/"),
        "a script cannot keep the candidate argv0; the kernel passes the descriptor path: {zero}"
    );
    fs::remove_dir_all(&dir).expect("the stub directory is removed");
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
fn a_non_executable_candidate_falls_through_to_the_next() {
    let fixture = Fixture::new("candidate-fallthrough");
    let plain = write_file(&fixture, "plain", "#!/bin/sh\nexit 0\n", 0o644);
    let real = write_file(&fixture, "real", "#!/bin/sh\nexit 0\n", 0o755);
    let candidates = [plain, real.clone()];
    let held = candidates
        .iter()
        .find_map(|candidate| Held::open(candidate).ok())
        .expect("the first candidate that opens is held");
    assert_eq!(
        held.path(),
        real,
        "the non-executable first candidate is skipped"
    );
}

#[cfg(unix)]
#[test]
fn dir_is_safe_truth_table() {
    const EUID: u32 = 1000;
    assert!(dir_is_safe(0, 0o755, EUID), "root-owned 0755 is safe");
    assert!(dir_is_safe(EUID, 0o755, EUID), "euid-owned 0755 is safe");
    assert!(
        dir_is_safe(EUID, 0o775, EUID),
        "euid-owned 0775 is safe, as a Homebrew install directory"
    );
    assert!(dir_is_safe(EUID, 0o700, EUID), "euid-owned 0700 is safe");
    assert!(
        !dir_is_safe(EUID + 1, 0o755, EUID),
        "another user's 0755 is unsafe"
    );
    assert!(!dir_is_safe(0, 0o775, EUID), "root-owned 0775 is unsafe");
    assert!(!dir_is_safe(0, 0o757, EUID), "root-owned 0757 is unsafe");
    assert!(dir_is_safe(0, 0o700, EUID), "root-owned 0700 is safe");
}

#[cfg(unix)]
#[test]
fn file_is_safe_truth_table() {
    const EUID: u32 = 1000;
    assert!(file_is_safe(0, 0o755, EUID), "root-owned 0755 is safe");
    assert!(file_is_safe(EUID, 0o755, EUID), "euid-owned 0755 is safe");
    assert!(
        !file_is_safe(EUID + 1, 0o755, EUID),
        "another user's 0755 is unsafe"
    );
    assert!(
        !file_is_safe(EUID, 0o775, EUID),
        "euid-owned 0775 is unsafe"
    );
    assert!(
        !file_is_safe(EUID, 0o757, EUID),
        "euid-owned 0757 is unsafe"
    );
}
