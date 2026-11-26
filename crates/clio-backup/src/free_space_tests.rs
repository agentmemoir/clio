// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the verify pass's free-space gate:
//! the `df` report parser, the descriptor the gate pins before it executes
//! anything, and every arm of the check that can be exercised without a real
//! filesystem.
//!
//! ## Owns
//! - The parser assertions and the injected `df` outcomes
//! - The pin assertions: the descriptor survives a swap of its path, it stays
//!   inheritable, and a candidate that is not a regular file is skipped
//!
//! ## Does not own
//! - The call that runs `df` itself (see `free_space`)
//!
//! ## Boundary
//!
//! Keep this file focused on how the free-space gate answers.

use super::*;
use std::process::{ExitStatus, Output};

/// One `df -P -k` outcome.
fn output(code: i32, stdout: &str) -> Output {
    Output {
        status: exit_status(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

#[cfg(unix)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    ExitStatus::from_raw(code)
}

#[cfg(not(unix))]
fn exit_status(code: i32) -> ExitStatus {
    let mut command = std::process::Command::new("cmd");
    command.arg("/C").arg(format!("exit {code}"));
    command.status().expect("the test host runs its own shell")
}

/// A POSIX report: header, then filesystem, 1024-blocks, used, available,
/// capacity, mount point.
const REPORT: &str = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      /dev/disk1 100000 40000 60000 40% /\n";

#[test]
fn a_posix_report_names_the_available_blocks() {
    assert_eq!(parse_available_kb(REPORT), Some(60_000));
}

#[test]
fn a_report_without_a_data_line_has_no_answer() {
    assert_eq!(
        parse_available_kb("Filesystem 1024-blocks Used Available\n"),
        None
    );
}

#[test]
fn a_report_with_a_non_numeric_available_column_has_no_answer() {
    let text = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk1 1 2 ? 3% /\n";
    assert_eq!(parse_available_kb(text), None);
}

#[test]
fn a_successful_df_answers_in_bytes() {
    let bytes = run_available_bytes(Path::new("/"), &|_| Ok(output(0, REPORT))).expect("an answer");
    assert_eq!(bytes, 60_000 * 1024);
}

#[test]
fn a_failing_df_is_an_internal_error_not_room() {
    let error = run_available_bytes(Path::new("/"), &|_| Ok(output(1, ""))).expect_err("a refusal");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("`df` exited"), "{}", error.message);
}

#[test]
fn a_df_that_cannot_run_is_an_internal_error() {
    let error = run_available_bytes(Path::new("/"), &|_| {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"))
    })
    .expect_err("a refusal");
    assert_eq!(error.code, ErrorCode::Internal);
    assert!(error.message.contains("gone"), "{}", error.message);
}

#[test]
fn an_unreadable_df_report_is_an_internal_error() {
    let error = run_available_bytes(Path::new("/"), &|_| Ok(output(0, "garbage\n")))
        .expect_err("a refusal");
    assert_eq!(error.code, ErrorCode::Internal);
}

/// Whether this host has a `df` at one of the fixed absolute paths. A host
/// without one is a real layout, so a live check is skipped rather than failed.
fn host_has_df() -> bool {
    DF_PATHS.iter().any(|path| Path::new(path).is_file())
}

#[test]
fn no_payload_bytes_needs_no_check() {
    require_room(Path::new("/definitely/not/a/directory"), 0).expect("nothing to reserve");
}

#[test]
fn more_bytes_than_this_host_holds_is_refused() {
    if !host_has_df() {
        return;
    }
    let error = require_room(Path::new("/"), u64::MAX).expect_err("no host holds u64::MAX bytes");
    assert_eq!(error.code, ErrorCode::QuotaExceeded);
    assert!(
        error.message.contains("needs") && error.message.contains("free"),
        "the refusal names the need and the free space: {}",
        error.message
    );
}

/// A scratch directory of this test's own, under the system temporary
/// directory, removed when the test ends, including when it panics.
///
/// The crate carries no `tempfile` dependency, so the name is the process id
/// plus a counter and nothing here touches a fixed path.
#[cfg(unix)]
struct Scratch {
    path: PathBuf,
}

#[cfg(unix)]
impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("clio-free-space-{}-{serial}", std::process::id()));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self { path }
    }

    /// The scratch path as the candidate text `pin_df_from` takes.
    fn candidate(&self, name: &str) -> String {
        self.path
            .join(name)
            .to_str()
            .expect("a utf-8 scratch path")
            .to_owned()
    }
}

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Whether this host names an open descriptor as a path, which a pin needs.
#[cfg(unix)]
fn host_names_descriptors() -> bool {
    Path::new(FD_LINK_DIR).is_dir()
}

/// A regular-file candidate: a copy of this test binary inside `scratch`.
#[cfg(unix)]
fn candidate_in(scratch: &Scratch, name: &str) -> PathBuf {
    let path = scratch.path.join(name);
    std::fs::copy(std::env::current_exe().expect("this test binary"), &path)
        .expect("a copy of this test binary");
    path
}

/// The `(device, inode)` a descriptor names.
#[cfg(unix)]
fn descriptor_identity(fd: &rustix::fd::OwnedFd) -> (u64, u64) {
    let stat = rustix::fs::fstat(fd).expect("a stat of the pinned descriptor");
    (stat.st_dev as u64, stat.st_ino as u64)
}

/// The `(device, inode)` a path names.
#[cfg(unix)]
fn path_identity(path: &Path) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).expect("a stat of the path");
    (metadata.dev(), metadata.ino())
}

#[cfg(unix)]
#[test]
fn pinned_descriptor_survives_a_path_swap() {
    if !host_names_descriptors() {
        return;
    }
    let scratch = Scratch::new();
    let path = candidate_in(&scratch, "df-candidate");
    let candidate = scratch.candidate("df-candidate");
    let pinned = pin_df_from(&[candidate.as_str()])
        .expect("reading a candidate is not an error")
        .expect("the copy is a regular file");
    let pinned_identity = descriptor_identity(&pinned.fd);

    // A fresh file is moved over the candidate. Its inode cannot be the pinned
    // one, because the pinned inode is still live through the descriptor.
    let replacement = scratch.path.join("replacement");
    std::fs::write(&replacement, b"# not a program\n").expect("the replacement is written");
    std::fs::rename(&replacement, &path).expect("the candidate path now names the replacement");

    assert_eq!(
        descriptor_identity(&pinned.fd),
        pinned_identity,
        "the pinned descriptor still names the file whose type was checked"
    );
    assert_ne!(
        path_identity(&path),
        pinned_identity,
        "the swap did replace what the candidate path names"
    );
    assert!(
        pinned.program.starts_with(FD_LINK_DIR),
        "the exec target is the descriptor, not the path: {}",
        pinned.program.display()
    );
    assert_ne!(
        pinned.program, path,
        "the candidate path is never the exec target"
    );
    assert_eq!(
        pinned.source, path,
        "argv[0] stays the candidate path that was opened"
    );
}

#[cfg(unix)]
#[test]
fn pinned_descriptor_is_not_close_on_exec() {
    if !host_names_descriptors() {
        return;
    }
    let scratch = Scratch::new();
    candidate_in(&scratch, "df-candidate");
    let candidate = scratch.candidate("df-candidate");
    let pinned = pin_df_from(&[candidate.as_str()])
        .expect("reading a candidate is not an error")
        .expect("the copy is a regular file");

    let flags = rustix::io::fcntl_getfd(&pinned.fd).expect("the descriptor flags");
    assert!(
        !flags.contains(rustix::io::FdFlags::CLOEXEC),
        "the child must inherit the descriptor it is executed from: {flags:?}"
    );
}

#[cfg(unix)]
#[test]
fn pin_skips_a_directory_candidate() {
    let scratch = Scratch::new();
    std::fs::create_dir(scratch.path.join("df")).expect("a candidate directory");
    let candidate = scratch.candidate("df");

    let pinned = pin_df_from(&[candidate.as_str()]).expect("skipping a candidate is not an error");
    assert!(
        pinned.is_none(),
        "a directory is not a program this gate will execute"
    );
}

/// A FIFO needs `mkfifoat`, which Apple, Redox, and WASI do not have.
#[cfg(all(
    unix,
    not(target_vendor = "apple"),
    not(target_os = "redox"),
    not(target_os = "wasi")
))]
#[test]
fn pin_skips_a_fifo_candidate_without_blocking() {
    let scratch = Scratch::new();
    let fifo = scratch.path.join("df");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        fifo.as_path(),
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .expect("a candidate FIFO with no writer");
    let candidate = scratch.candidate("df");

    // A blocking open of a FIFO with no writer would never return, so a hang
    // here is the failure this test exists to catch.
    let pinned = pin_df_from(&[candidate.as_str()]).expect("skipping a candidate is not an error");
    assert!(
        pinned.is_none(),
        "a FIFO is not a program this gate will execute"
    );
}

/// A host without the descriptor directory answers from the file-system
/// statistics, in whole fragments of the file system.
#[cfg(unix)]
#[test]
fn the_statistics_answer_is_in_whole_file_system_fragments() {
    let scratch = Scratch::new();
    let space = rustix::fs::statvfs(scratch.path.as_path()).expect("the scratch file system");
    assert!(
        space.f_bavail > 0,
        "the scratch file system reports room, so the next assertion can be read"
    );

    let bytes = available_bytes_without_df(scratch.path.as_path()).expect("an answer");
    assert!(
        bytes > 0,
        "the fallback reports the room the file system has"
    );
    assert_eq!(
        bytes % space.f_frsize,
        0,
        "the answer of {bytes} bytes is a whole number of fragments"
    );
}

/// A host with no `df` at any fixed path is refused by name, and the refusal
/// lists every path that was tried.
#[test]
fn a_missing_df_names_every_path_that_was_tried() {
    let error = missing_df_error();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    let message = error.to_string();
    for path in DF_PATHS {
        assert!(message.contains(path), "{path} is named in: {message}");
    }
}
