// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the descriptor the restore pre-flight
//! pins before it executes anything: where that descriptor sits, that the child
//! executes the file it holds rather than whatever the path names later, and
//! which answer the chooser gives.
//!
//! ## Owns
//! - The behavioural assertions: a pinned `df` script answers, and the same
//!   script replaced at its path does not answer for it
//! - The descriptor invariants: fd 3 or higher, inheritable, and a candidate
//!   that is not a regular file is skipped
//! - Both arms of the chooser between a pinned `df` and the statistics
//!
//! ## Does not own
//! - The candidate list and the report parser (see `restore_space_df_tests`)
//! - The pre-flight's own grouping and refusal message (see
//!   `restore_space_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Changes to how a `df`
//! report is parsed belong in the sibling `df` test module, not here.

use super::*;
use crate::restore_space::restore_space_pin::*;
#[cfg(unix)]
use std::process::Output;

/// The roots a scratch directory may live in, most preferred first.
///
/// `std::env::temp_dir()` is last and is never trusted. A `noexec` mount
/// accepts the write and the mode change and then refuses to execute the file,
/// even through `/proc/self/fd/<n>`, so a scratch directory there cannot hold
/// the `df` these tests run; CI points `TMPDIR` at exactly that. The first root
/// is the directory this test binary was loaded from, which is executable by
/// construction, because the kernel already executed a file from it. The second
/// is the workspace `target` directory, which sits on the build volume.
#[cfg(unix)]
fn scratch_roots() -> [PathBuf; 3] {
    let beside_the_binary = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    [
        beside_the_binary,
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        std::env::temp_dir(),
    ]
}

/// Whether `root` can hold a file and execute it, proved by running one.
///
/// Nothing short of an actual exec tells a `noexec` mount from a root these
/// tests may use, so this writes the smallest possible `#!/bin/sh` script, marks
/// it executable and runs it.
#[cfg(unix)]
fn executes_a_file(root: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    if root.as_os_str().is_empty() || std::fs::create_dir_all(root).is_err() {
        return false;
    }
    let probe = root.join(format!(
        "clio-exec-probe-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let prepared = std::fs::write(&probe, "#!/bin/sh\nexit 0\n")
        .and_then(|()| std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)));
    let ran = prepared.is_ok()
        && std::process::Command::new(&probe)
            .status()
            .is_ok_and(|status| status.success());
    let _ = std::fs::remove_file(&probe);
    ran
}

/// A scratch directory removed when the guard drops.
///
/// It lives in the first root on this host that can execute a file. `None` where
/// every root refuses the probe: that is a host precondition, not a failure, so
/// the caller skips the test.
#[cfg(unix)]
struct Scratch(PathBuf);

#[cfg(unix)]
impl Scratch {
    fn new(tag: &str) -> Option<Self> {
        let name = crate::test_unique::unique(&format!("clio-restore-pin-{tag}"));
        scratch_roots().into_iter().find_map(|root| {
            let path = root.join(&name);
            if !executes_a_file(&root) || std::fs::create_dir_all(&path).is_err() {
                return None;
            }
            Some(Self(path))
        })
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

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Report why the calling test is skipped: no root on this host can execute a
/// file, which is a host precondition rather than a failure.
///
/// The message names the precondition so a silent skip is not mistaken for a
/// pass.
#[cfg(unix)]
fn skip_without_an_executable_root() {
    eprintln!(
        "skipping: no scratch root on this host can execute a file, and a pinned df must run \
         (precondition: a directory this test may write to and execute a script from; the \
         system temporary directory is not assumed executable)"
    );
}

/// Take a scratch directory into `$name`, or skip the calling test where this
/// host has no root that can execute a file.
#[cfg(unix)]
macro_rules! scratch {
    ($name:ident, $tag:expr) => {
        let Some($name) = Scratch::new($tag) else {
            return skip_without_an_executable_root();
        };
    };
}

/// Whether this host names an open descriptor as a path, which a pin needs.
#[cfg(unix)]
fn host_names_descriptors() -> bool {
    Path::new(FD_LINK_DIR).is_dir()
}

/// Write an executable `df -P -k` script at `name` inside `scratch` whose
/// Available column is `available_kb`, and answer with its path.
#[cfg(unix)]
fn df_script(scratch: &Scratch, name: &str, available_kb: u64) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = scratch.0.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             echo 'Filesystem 1024-blocks Used Available Capacity Mounted on'\n\
             echo '/dev/scratch 1000 1 {available_kb} 1% /'\n"
        ),
    )
    .expect("a df script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("an executable df script");
    path
}

/// The available-blocks column of a script's report, read the way the
/// pre-flight reads a real one.
#[cfg(unix)]
fn usable_kb(output: &Output) -> Option<u64> {
    parse_available_kb(&String::from_utf8_lossy(&output.stdout))
}

/// A successful `df -P -k` outcome, for the injected runner the chooser takes.
#[cfg(unix)]
fn output(stdout: &str) -> Output {
    use std::os::unix::process::ExitStatusExt as _;

    Output {
        status: std::process::ExitStatus::from_raw(0),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

/// A runner that fails the test if the arm it is handed to ever runs a `df`.
#[cfg(unix)]
fn no_df(_dir: &Path) -> std::io::Result<Output> {
    panic!("the statistics arm must not execute a df")
}

/// The `df` the pre-flight runs is the one it pinned, not whatever the
/// candidate path names by the time the child is executed.
#[cfg(unix)]
#[test]
fn the_pinned_df_runs_the_descriptor_not_the_swapped_path() {
    if !host_names_descriptors() || !Path::new("/bin/sh").is_file() {
        return;
    }
    scratch!(scratch, "behavioural");
    let candidate = scratch.candidate("df");
    let path = df_script(&scratch, "df", 424_242);

    // The production seam on a candidate this host offers: the first script
    // answers, and the answer is its own column.
    let output = df_output_from(scratch.0.as_path(), &[candidate.as_str()])
        .expect("the pinned descriptor runs");
    assert_eq!(usable_kb(&output), Some(424_242));

    // The pin is taken before the swap, so the descriptor it holds is the only
    // thing that can still answer with the first script's column.
    let pinned = pin_df_from(&[candidate.as_str()]).expect("the script is a regular file");
    df_script(&scratch, "swapped", 999_999);
    std::fs::rename(scratch.0.join("swapped"), &path)
        .expect("the second script lands on the candidate path");
    let output = run_pinned_df(&pinned, scratch.0.as_path()).expect("the pinned descriptor runs");
    assert_eq!(
        usable_kb(&output),
        Some(424_242),
        "the child executed the path rather than the descriptor it was pinned from"
    );

    // Negative control: the swap really did replace the program at the path, so
    // the assertion above is about the descriptor and not about a script that
    // never changed.
    let swapped = std::process::Command::new(&path)
        .output()
        .expect("the second script runs by path");
    assert_eq!(
        usable_kb(&swapped),
        Some(999_999),
        "the candidate path still names the first script, so this test proves nothing"
    );
}

/// The descriptor the child executes must be inheritable, which is why the exec
/// step clears the flag the duplicate is created with.
#[cfg(unix)]
#[test]
fn the_descriptor_is_inheritable_by_the_child_it_is_executed_from() {
    if !host_names_descriptors() {
        return;
    }
    scratch!(scratch, "inherit");
    let candidate = scratch.candidate("df");
    df_script(&scratch, "df", 424_242);
    let pinned = pin_df_from(&[candidate.as_str()]).expect("the script is a regular file");

    let inherited = rustix::io::fcntl_getfd(&pinned.fd).expect("the descriptor flags");
    assert!(
        inherited.contains(rustix::io::FdFlags::CLOEXEC),
        "the duplicate is created close-on-exec, so a dropped pin cannot leak: {inherited:?}"
    );
    inherit_for_child(&pinned.fd).expect("the descriptor flags");
    let cleared = rustix::io::fcntl_getfd(&pinned.fd).expect("the descriptor flags");
    assert!(
        !cleared.contains(rustix::io::FdFlags::CLOEXEC),
        "the child must inherit the descriptor it is executed from: {cleared:?}"
    );
}

#[cfg(unix)]
#[test]
fn pin_skips_a_directory_candidate() {
    scratch!(scratch, "directory");
    std::fs::create_dir(scratch.0.join("df")).expect("a candidate directory");
    let candidate = scratch.candidate("df");

    let pinned = pin_df_from(&[candidate.as_str()]);
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
    scratch!(scratch, "fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        scratch.0.join("df").as_path(),
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .expect("a candidate FIFO with no writer");
    let candidate = scratch.candidate("df");

    // A blocking open of a FIFO with no writer would never return, so a hang
    // here is the failure this test exists to catch.
    let pinned = pin_df_from(&[candidate.as_str()]);
    assert!(
        pinned.is_none(),
        "a FIFO is not a program this pre-flight will execute"
    );
}

/// A host that cannot name a descriptor answers from the statistics, in whole
/// fragments of the file system.
#[cfg(unix)]
#[test]
fn the_statistics_answer_is_in_whole_file_system_fragments() {
    scratch!(scratch, "statistics");
    let space = rustix::fs::statvfs(scratch.0.as_path()).expect("the scratch file system");
    if space.f_bavail == 0 {
        // Host-supplied precondition: the temp filesystem has no room, so there
        // is no non-zero answer to assert the shape of. The `f_frsize == 0`
        // guard and the multiplier are covered by the chooser test below.
        return;
    }
    let fragment = if space.f_frsize == 0 {
        space.f_bsize
    } else {
        space.f_frsize
    };

    let bytes = available_bytes_without_df(scratch.0.as_path()).expect("an answer");
    assert!(bytes > 0, "the statistics find the room the mount has");
    assert_eq!(
        bytes % fragment,
        0,
        "the answer of {bytes} bytes is a whole number of fragments of {fragment} bytes"
    );
}

/// The chooser reads the statistics, and runs nothing, where no descriptor can
/// be named.
#[cfg(unix)]
#[test]
fn the_chooser_reads_the_statistics_where_no_descriptor_can_be_named() {
    scratch!(scratch, "chooser-statistics");
    let space = rustix::fs::statvfs(scratch.0.as_path()).expect("the scratch file system");
    let fragment = if space.f_frsize == 0 {
        space.f_bsize
    } else {
        space.f_frsize
    };
    // `no_df` panics, so reaching the next line at all is the assertion that this
    // arm ran no program. The figure is the statistics, in whole fragments; its
    // exact value is not compared, because a second reading of a live filesystem
    // is a different snapshot rather than the same one.
    let chosen = available_bytes_with(scratch.0.as_path(), false, &no_df).expect("an answer");
    if fragment > 0 {
        assert_eq!(
            chosen % fragment,
            0,
            "the answer of {chosen} bytes is a whole number of fragments of {fragment} bytes"
        );
    }
    if space.f_bavail > 0 {
        assert!(chosen > 0, "the statistics find the room the mount has");
    }
}

/// The chooser runs the `df` it is given where a descriptor can be named.
#[cfg(unix)]
#[test]
fn the_chooser_runs_the_df_where_a_descriptor_can_be_named() {
    scratch!(scratch, "chooser-df");
    let report = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                  /dev/scratch 1000 1 424242 1% /\n";
    let chosen = available_bytes_with(scratch.0.as_path(), true, &|_| Ok(output(report)))
        .expect("an answer");
    assert_eq!(
        chosen,
        424_242 * 1024,
        "the arm that can name a descriptor answers from the df it runs"
    );
}

/// The descriptor the gate keeps never sits on a standard stream.
///
/// A host may start with 0, 1 or 2 closed, and a child's stdio is installed over
/// exactly those slots: a pin that kept one of them would name a pipe instead of
/// the candidate, and the exec would fail. The slot cannot be freed here —
/// closing a standard stream in this process would break the test that is
/// running, and moving a descriptor onto one of those slots needs an owned
/// descriptor for it, which only `unsafe` can build in a crate that forbids it.
/// So the fence is asserted where it is observable: the pin does not reuse the
/// descriptor its own open took, and it carries `FD_CLOEXEC`, which only the
/// duplicate carries.
#[cfg(unix)]
#[test]
fn the_kept_descriptor_is_fenced_above_the_standard_streams() {
    use std::os::fd::AsRawFd as _;

    if !host_names_descriptors() || !Path::new("/bin/sh").is_file() {
        return;
    }
    scratch!(scratch, "fenced");
    let candidate = scratch.candidate("df");
    df_script(&scratch, "df", 424_242);

    // The probe takes the lowest free descriptor and gives it back, so this is
    // the slot the pin's own open lands on.
    let probe = std::fs::File::open("/dev/null").expect("/dev/null opens");
    let opened = probe.as_raw_fd();
    drop(probe);

    let pinned = pin_df_from(&[candidate.as_str()]).expect("the script is a regular file");
    let raw = pinned.fd.as_raw_fd();
    assert_ne!(
        raw, opened,
        "the pin kept the descriptor its own open took at fd {opened}, so a closed stdio slot \
         would swallow it"
    );
    assert!(
        raw >= 3,
        "the pin is at fd {raw}, which a child's stdio is installed over"
    );
    let output = run_pinned_df(&pinned, Path::new("/")).expect("the pinned descriptor runs");
    assert_eq!(
        usable_kb(&output),
        Some(424_242),
        "the descriptor the gate kept is executable"
    );
}
