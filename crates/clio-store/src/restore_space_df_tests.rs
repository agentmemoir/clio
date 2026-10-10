// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing how the restore pre-flight finds `df`:
//! the fixed absolute candidate list, the error when no candidate exists, and
//! the guarantee that a program the pre-flight did not offer is never run.
//!
//! ## Owns
//! - The `df` resolver assertions: fixed paths, a decoy that must lose, and the
//!   no-candidate-found arm
//!
//! ## Does not own
//! - The pre-flight's own grouping and refusal message (see
//!   `restore_space_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to the `df` report
//! parser belong in the sibling file, not here.

use super::*;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-restore-df-{tag}"
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

#[test]
fn no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried() {
    // Four paths inside a fresh scratch directory, so none of them can exist on
    // any host. This is the arm a host with no `df` in its fixed paths takes,
    // exercised on every host.
    let scratch = Scratch::new("absent");
    let candidates: Vec<String> = ["one", "two", "three", "four"]
        .iter()
        .map(|name| scratch.0.join(name).display().to_string())
        .collect();
    let offered: Vec<&str> = candidates.iter().map(String::as_str).collect();

    let error = df_command_from(&scratch.0, &offered)
        .expect_err("none of these paths can exist, so there is no df to run");
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
    // A script that would answer "there is room" for anything. If the pre-flight
    // ever searched `PATH`, a hostile entry like this one would be picked up.
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         echo 'Filesystem 1024-blocks Used Available Capacity Mounted on'\n\
         echo 'fake 9000000000000000 0 9000000000000000 0% /'\n",
    )
    .expect("seed the fake df");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
        .expect("make the fake df executable");
    assert!(fake.is_file(), "the fake df is a real executable file");

    // Control: the resolver does return an existing file when one is offered, so
    // the assertion below is not passing because it always answers `None`.
    let offered = fake.to_str().expect("scratch path is text");
    assert_eq!(
        first_existing_df(&[offered]).as_deref(),
        Some(fake.as_path()),
        "the resolver honours the list it is given"
    );

    // Host-dependent: on a host with no `df` at any fixed path there is nothing
    // to choose, so the choice is not a decision this host can make.
    if !host_has_df() {
        return;
    }
    let chosen = first_existing_df(&DF_PATHS).expect("this host has a df");
    assert_ne!(
        chosen,
        fake,
        "the fixed list resolved to the fake df at {}",
        fake.display()
    );
    assert!(
        DF_PATHS
            .iter()
            .any(|candidate| Path::new(candidate) == chosen),
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
    let fake_text = fake.to_str().expect("scratch path is text");

    // What the resolver offers no protection against: it is a plain filter over
    // the list, so a decoy placed FIRST is chosen. The safety lives entirely in
    // the caller passing the fixed list, which is what the next assertions pin.
    assert_eq!(
        first_existing_df(&[fake_text]).as_deref(),
        Some(fake.as_path()),
        "the resolver honours the order of the list it is given"
    );

    // The decoy, offered with the fixed paths, is behind them all.
    let mut offered = DF_PATHS.to_vec();
    offered.push(fake_text);
    let chosen = df_command_from(&scratch.0, &offered)
        .ok()
        .map(|command| command.get_program().to_owned());
    // Host-dependent only in the positive branch: a host with no `df` at any
    // fixed path chooses nothing, and choosing nothing also means the fake lost.
    // The claim under test, that the fake is not chosen, is checked everywhere.
    if let Some(program) = chosen {
        assert_ne!(
            program,
            fake.as_os_str(),
            "the fake df won over the fixed paths: {program:?}"
        );
        assert!(
            DF_PATHS
                .iter()
                .any(|candidate| Path::new(candidate) == program),
            "{program:?} is not one of the fixed paths {DF_PATHS:?}"
        );
    }

    // And the host-free reason it cannot win in production, whatever the order:
    // no candidate in the production list lives inside a scratch directory, so
    // a file planted there is not a candidate at all.
    assert!(
        !DF_PATHS
            .iter()
            .any(|candidate| Path::new(candidate).starts_with(&scratch.0)),
        "a production candidate must never be a file a test or a user planted in {}: {DF_PATHS:?}",
        scratch.0.display()
    );
}

#[test]
fn the_resolver_never_falls_back_to_a_bare_program_name() {
    // Nothing exists at these paths, so the resolver has nothing to return. If
    // any code path fell back to a bare `df`, it would search the environment
    // and this would succeed instead of reporting not found.
    let scratch = Scratch::new("bare");
    let absent: Vec<String> = ["bin", "usr-bin", "usr-sbin", "sbin"]
        .iter()
        .map(|dir| scratch.0.join(dir).join("df").display().to_string())
        .collect();
    let offered: Vec<&str> = absent.iter().map(String::as_str).collect();
    let error = df_command_from(&scratch.0, &offered)
        .expect_err("an empty list of candidates must not fall back to a bare `df`");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");

    // The same list the production path uses is absolute, so the program the
    // production path builds is absolute too.
    for candidate in DF_PATHS {
        assert!(
            Path::new(candidate).is_absolute(),
            "the production candidate {candidate} must be an absolute path"
        );
    }
    // Limit of this test: it cannot put a fake `df` on `PATH`, because
    // `std::env::set_var` is process-wide and `unsafe` in Rust 2024, and it does
    // not need to. A resolver that searched `PATH` would be caught by
    // `a_fake_df_in_a_scratch_directory_is_never_chosen` only if `PATH` held the
    // scratch directory, which it never does, so no test here can prove the
    // absence of a `PATH` search by mutating the environment. What is proved is
    // narrower and still the part that can fail: the resolver takes its whole
    // answer from the list it is given.
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
    // Host-dependent: a host with no `df` at any fixed path cannot answer. The
    // `NotFound` path is covered host-free by
    // `no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried`.
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

#[test]
fn the_df_command_names_a_fixed_absolute_program() {
    // Host-dependent: with no `df` at any fixed path there is no program to
    // name. The not-found arm is covered host-free by
    // `no_df_at_any_offered_path_is_a_not_found_that_names_every_path_tried`.
    if !host_has_df() {
        return;
    }
    let scratch = Scratch::new("command");
    let command = df_command(&scratch.0)
        .unwrap_or_else(|error| panic!("no `df` at {DF_PATHS:?} could be run: {error}"));
    let program = command.get_program();
    assert!(
        Path::new(program).is_absolute(),
        "the program {} is relative, so the operating system resolves it against the current \
         directory or PATH",
        program.to_string_lossy()
    );
    assert!(
        DF_PATHS
            .iter()
            .any(|candidate| Path::new(candidate) == Path::new(program)),
        "the program {} is not one of the fixed paths {DF_PATHS:?}",
        program.to_string_lossy()
    );
}
