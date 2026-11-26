// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing how the restore pre-flight finds and
//! pins `df`: the fixed absolute candidate list, the error when no candidate
//! exists, the descriptor pinned from the first regular file, and the guarantee
//! that a program the pre-flight did not offer is never run.
//!
//! ## Owns
//! - The `df` assertions: fixed paths, decoys that must lose, the
//!   no-candidate-found arm, and the statistics answer used where nothing can be
//!   executed
//! - The pin assertions: the descriptor keeps the file whose type was checked
//!   across a swap of its path, it stays inheritable, and a candidate that is
//!   not a regular file is skipped
//!
//! ## Does not own
//! - The pre-flight's own grouping and refusal message (see
//!   `restore_space_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to the `df` report
//! parser belong in the sibling `restore_space_tests`, not here.

use super::*;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-restore-df-{tag}"
        )))
    }

    /// The path `name` inside this directory, as the candidate text a pin takes.
    fn candidate(&self, name: &str) -> String {
        self.0
            .join(name)
            .to_str()
            .expect("a utf-8 scratch path")
            .to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether this host pins a `df` at one of the fixed absolute paths.
///
/// A host that does not is a real layout, not a broken one, so the assertions
/// that need a live `df` are skipped rather than failed. The behaviour that arm
/// would have covered is covered host-free by
/// `no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried`.
fn host_has_df() -> bool {
    resolve_df().is_some()
}

/// One pebibyte, far above any single filesystem and far below `u64::MAX`, so a
/// parsed column that lands outside it is a misparse rather than a real disk.
const PEBIBYTE: u64 = 1 << 50;

/// A regular-file candidate: a copy of this test binary inside `scratch`.
#[cfg(unix)]
fn candidate_in(scratch: &Scratch, name: &str) -> PathBuf {
    let path = scratch.0.join(name);
    std::fs::copy(std::env::current_exe().expect("this test binary"), &path)
        .expect("a copy of this test binary");
    path
}

#[test]
fn every_df_candidate_is_an_absolute_path() {
    for candidate in DF_PATHS {
        assert!(
            Path::new(candidate).is_absolute(),
            "`df` candidate {candidate} is relative; the pre-flight must only ever run an absolute \
             path"
        );
    }
}

#[cfg(unix)]
#[test]
fn no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried() {
    // Four paths inside a fresh scratch directory, so none of them can exist on
    // any host. This is the arm a host with no `df` in its fixed paths takes,
    // exercised on every host.
    let scratch = Scratch::new("absent");
    let candidates: Vec<String> = ["one", "two", "three", "four"]
        .iter()
        .map(|name| scratch.candidate(name))
        .collect();
    let offered: Vec<&str> = candidates.iter().map(String::as_str).collect();

    let error = pin_df(&offered).err().expect("no such candidate can exist");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::NotFound,
        "a host with no df must be reported as not found, not as room: {error}"
    );
    for candidate in &candidates {
        assert!(
            error.to_string().contains(candidate.as_str()),
            "the message names every path that was tried; {candidate} is missing from: {error}"
        );
    }
    assert!(
        !error.to_string().contains("PATH"),
        "the resolver never consults the environment, so the message must not mention PATH: \
         {error}"
    );
}

#[cfg(unix)]
#[test]
fn a_fake_df_in_a_scratch_directory_is_never_chosen() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("fake-df");
    let fake = scratch.0.join("df");
    // A script that would answer "there is room" for anything: if the
    // pre-flight ever searched `PATH`, a hostile entry like this would win.
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         echo 'Filesystem 1024-blocks Used Available Capacity Mounted on'\n\
         echo 'fake 9000000000000000 0 9000000000000000 0% /'\n",
    )
    .expect("seed the fake df");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake df executable");

    // Control: the pin does take an existing regular file when one is offered,
    // so the assertion below is not passing because it always answers nothing.
    let offered = scratch.candidate("df");
    let pinned = pin_df_from(&[offered.as_str()])
        .expect("reading a candidate is not an error")
        .expect("the fake df is a regular file");
    assert_eq!(pinned.source, fake, "the pin honours its list");

    // Host-dependent: on a host with no `df` at any fixed path there is nothing
    // to choose, so the choice is not a decision this host can make.
    if !host_has_df() {
        return;
    }
    let chosen = pin_df(&DF_PATHS).expect("this host has a df").source;
    assert_ne!(
        chosen,
        fake,
        "the fixed list resolved to the fake df at {}",
        fake.display()
    );
    assert!(
        DF_PATHS.iter().any(|c| Path::new(c) == chosen),
        "{chosen:?} is not one of the fixed paths {DF_PATHS:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_fake_df_loses_when_the_fixed_paths_are_offered_alongside_it() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("fake-loses");
    let fake = scratch.0.join("df");
    std::fs::write(&fake, "#!/bin/sh\nexit 0\n").expect("seed the fake df");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake df executable");
    let fake_text = scratch.candidate("df");

    // The pin is a plain filter over the list, so a decoy placed FIRST is
    // pinned. The safety lives in the caller passing the fixed list.
    let decoy = pin_df_from(&[fake_text.as_str()])
        .expect("reading a candidate is not an error")
        .expect("the fake df is a regular file");
    assert_eq!(decoy.source, fake, "the pin honours the order");

    // The decoy, offered with the fixed paths, is behind them all.
    let mut offered = DF_PATHS.to_vec();
    offered.push(fake_text.as_str());
    let chosen = pin_df(&offered).ok().map(|pinned| pinned.source);
    // Host-dependent only in the positive branch: a host with no `df` at any
    // fixed path chooses nothing, and choosing nothing also means the fake lost.
    if let Some(source) = chosen {
        assert_ne!(
            source, fake,
            "the fake df won over the fixed paths: {source:?}"
        );
        assert!(
            DF_PATHS.iter().any(|c| Path::new(c) == source),
            "{source:?} is not one of the fixed paths {DF_PATHS:?}"
        );
    }

    // And the host-free reason it cannot win, whatever the order: no production
    // candidate lives inside a scratch directory, so a planted file is not one.
    assert!(
        !DF_PATHS
            .iter()
            .any(|candidate| Path::new(candidate).starts_with(&scratch.0)),
        "a production candidate must never be a file a test or a user planted in {}: {DF_PATHS:?}",
        scratch.0.display()
    );
}

#[cfg(unix)]
#[test]
fn the_resolver_never_falls_back_to_a_bare_program_name() {
    // Nothing exists at these paths, so there is nothing to pin. A code path
    // that fell back to a bare `df` would search the environment and succeed
    // instead of reporting not found.
    let scratch = Scratch::new("bare");
    let absent: Vec<String> = ["bin", "usr-bin", "usr-sbin", "sbin"]
        .iter()
        .map(|dir| scratch.candidate(&format!("{dir}/df")))
        .collect();
    let offered: Vec<&str> = absent.iter().map(String::as_str).collect();
    let error = pin_df(&offered).err().expect("no candidate here can exist");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");

    // Limit of this test: it cannot put a fake `df` on `PATH`, because
    // `std::env::set_var` is process-wide and `unsafe` in Rust 2024. The two
    // sibling tests prove instead that the candidate list decides and that no
    // production candidate can be resolved by name.
}

#[test]
fn the_resolver_answers_with_the_first_fixed_path_that_exists() {
    let first_on_disk = DF_PATHS
        .iter()
        .find(|candidate| Path::new(candidate).is_file())
        .map(|candidate| PathBuf::from(*candidate));
    match (resolve_df(), first_on_disk) {
        (Some(chosen), Some(expected)) => assert_eq!(
            chosen, expected,
            "the resolver must return the earliest existing candidate"
        ),
        (None, None) => {}
        (Some(chosen), None) => {
            panic!("{chosen:?} was returned but no fixed path exists: {DF_PATHS:?}")
        }
        (None, Some(expected)) => {
            panic!(
                "{} exists but the resolver returned nothing: {DF_PATHS:?}",
                expected.display()
            )
        }
    }
}

#[test]
fn available_bytes_answers_from_a_fixed_path_on_this_host() {
    // Host-dependent: a host with no `df` at any fixed path answers from its
    // statistics instead, host-free in
    // `the_statistics_answer_is_in_whole_file_system_fragments`.
    if !host_has_df() {
        return;
    }
    let scratch = Scratch::new("fixed-path");
    let available = available_bytes(&scratch.0).unwrap_or_else(|error| {
        panic!(
            "no `df` at {DF_PATHS:?} could answer for {} on this host: {}",
            scratch.0.display(),
            error.message
        )
    });
    assert!(
        available > 0 && available < PEBIBYTE,
        "a live filesystem reported {available} available bytes, which is not plausible"
    );
}

/// Whether this host names an open descriptor as a path, which a pin needs.
#[cfg(unix)]
fn host_names_descriptors() -> bool {
    Path::new(FD_LINK_DIR).is_dir()
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
fn the_pinned_program_is_the_descriptor_link_not_the_path() {
    use std::os::fd::AsRawFd as _;

    // Host-dependent: with no `df` at any fixed path there is no program to pin.
    // The not-found arm is covered host-free by
    // `no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried`.
    if !host_has_df() || !host_names_descriptors() {
        return;
    }
    let pinned = pin_df(&DF_PATHS).expect("this host has a df");
    assert_eq!(
        pinned.program,
        PathBuf::from(format!("{FD_LINK_DIR}/{}", pinned.fd.as_raw_fd())),
        "the exec target is the pinned descriptor, not a path"
    );
    assert_ne!(
        pinned.program, pinned.source,
        "the candidate path is never the exec target"
    );
    assert!(
        DF_PATHS.iter().any(|c| Path::new(c) == pinned.source),
        "{} is not one of the fixed paths {DF_PATHS:?}",
        pinned.source.display()
    );
}

#[cfg(unix)]
#[test]
fn pinned_descriptor_survives_a_path_swap() {
    if !host_names_descriptors() {
        return;
    }
    let scratch = Scratch::new("swap");
    let candidate = scratch.candidate("df-candidate");
    let path = candidate_in(&scratch, "df-candidate");
    let pinned = pin_df_from(&[candidate.as_str()])
        .expect("reading a candidate is not an error")
        .expect("the copy is a regular file");
    let pinned_identity = descriptor_identity(&pinned.fd);

    // A fresh file is moved over the candidate. Its inode cannot be the pinned
    // one, because the pinned inode is still live through the descriptor.
    let replacement = scratch.0.join("replacement");
    std::fs::write(&replacement, b"# not a program\n").expect("the replacement is written");
    std::fs::rename(&replacement, &path).expect("the candidate path names the replacement now");

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
    assert_eq!(
        pinned.source,
        PathBuf::from(&candidate),
        "argv[0] stays the candidate path that was opened"
    );
}

#[cfg(unix)]
#[test]
fn pinned_descriptor_is_not_close_on_exec() {
    if !host_names_descriptors() {
        return;
    }
    let scratch = Scratch::new("inherit");
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
    let scratch = Scratch::new("directory");
    std::fs::create_dir(scratch.0.join("df")).expect("a candidate directory");
    let candidate = scratch.candidate("df");

    let pinned = pin_df_from(&[candidate.as_str()]).expect("skipping a candidate is not an error");
    assert!(
        pinned.is_none(),
        "a directory is not a program this pre-flight will execute"
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
    let scratch = Scratch::new("fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        scratch.0.join("df").as_path(),
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .expect("a candidate FIFO with no writer");
    let candidate = scratch.candidate("df");

    // A blocking open of a FIFO with no writer would never return, so a hang
    // here is the failure this test exists to catch.
    let pinned = pin_df_from(&[candidate.as_str()]).expect("skipping a candidate is not an error");
    assert!(
        pinned.is_none(),
        "a FIFO is not a program this pre-flight will execute"
    );
}

/// A host that cannot pin a descriptor answers from the statistics.
#[cfg(unix)]
#[test]
fn the_statistics_answer_is_in_whole_file_system_fragments() {
    let scratch = Scratch::new("statistics");
    let space = rustix::fs::statvfs(scratch.0.as_path()).expect("the scratch file system");
    assert!(
        space.f_bavail > 0,
        "the scratch file system reports room, so the next assertion can be read"
    );

    let bytes = available_bytes_without_df(scratch.0.as_path()).expect("an answer");
    assert!(bytes > 0, "the fallback finds the room");
    assert_eq!(
        bytes % space.f_frsize,
        0,
        "the answer of {bytes} bytes is a whole number of fragments"
    );
}
