// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the manual writer: the one-page-per-leaf render with its
//! index, the sorted deterministic write order, the replace-only-generated
//! rule, and the symlink, special-file, collision, and traversal refusals.
//!
//! ## Owns
//! - The `Scratch` unique temporary directory guard
//! - Assertions over `render_manual`, `write_manual`, `check_target`, and the
//!   incomplete-write error shape
//!
//! ## Does not own
//! - The writer under test (the `cli_manual_write` module)
//! - The request validation (see `cli_top_level_markdown_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the bounded manual writer's write order and refusals.

use super::*;

/// One test's own directory, removed when the guard drops. No other test
/// shares it. The pattern is `agent_harness_write_tests`' `Scratch`: pid plus
/// a counter and nanos keep parallel tests apart, and the drop removes the
/// directory on panic too.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = unix_nanos();
        // Canonicalize the temp root before joining: the writer refuses every
        // symlink path component, and on macOS the system temp directory sits
        // behind symlinks (`/var` and `/tmp` point into `/private`), which
        // would fail the successful writes below for an environmental reason.
        let base = fs::canonicalize(std::env::temp_dir()).expect("the temp root resolves");
        let root = base.join(format!(
            "clio-manual-write-{label}-{}-{nanos}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("scratch directory can be created");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The first line of `path`'s bytes.
fn first_line(path: &Path) -> Vec<u8> {
    let bytes = fs::read(path).expect("the file can be read");
    match bytes.iter().position(|byte| *byte == b'\n') {
        Some(end) => bytes[..end].to_vec(),
        None => bytes,
    }
}

#[test]
fn the_manual_holds_one_page_per_registered_leaf_plus_the_index() {
    let manual = render_manual();
    let leaves = cli_command_registry::all().count();
    assert_eq!(manual.pages.len(), leaves, "one page per leaf");
    let mut files: Vec<&str> = manual.pages.iter().map(|page| page.file.as_str()).collect();
    files.push(manual.index.file.as_str());
    let mut unique = files.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), files.len(), "every page name is unique");
    assert!(
        files.contains(&PageMap::index_page()),
        "the manual holds the index"
    );
}

#[test]
fn the_render_is_deterministic_sorted_and_marks_every_file() {
    let first = render_manual();
    let second = render_manual();
    let bytes = |manual: &Manual| -> Vec<(String, String)> {
        manual
            .write_order()
            .map(|page| (page.file.clone(), page.bytes.clone()))
            .collect()
    };
    assert_eq!(bytes(&first), bytes(&second), "the render is repeatable");
    let mut files: Vec<&str> = first.pages.iter().map(|page| page.file.as_str()).collect();
    files.sort_unstable();
    let in_order: Vec<&str> = first.pages.iter().map(|page| page.file.as_str()).collect();
    assert_eq!(in_order, files, "the pages render sorted by page name");
    for page in first.write_order() {
        assert_eq!(
            first_line_of_bytes(page.bytes.as_bytes()),
            GENERATED_MARKER.as_bytes(),
            "{} opens with the generated-by marker",
            page.file
        );
    }
}

/// The first line of `bytes`.
fn first_line_of_bytes(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|byte| *byte == b'\n') {
        Some(end) => &bytes[..end],
        None => bytes,
    }
}

#[test]
fn write_manual_writes_every_page_and_the_index_into_a_new_directory() {
    let scratch = Scratch::new("write");
    let out = scratch.path("manual");
    write_manual(&out).expect("the manual writes into a new directory");
    let manual = render_manual();
    for page in manual.write_order() {
        let path = out.join(&page.file);
        assert_eq!(
            first_line(&path),
            GENERATED_MARKER.as_bytes(),
            "{} was written with the marker",
            page.file
        );
    }
    let index = fs::read_to_string(out.join("index.md")).expect("the index was written");
    for page in &manual.pages {
        assert!(
            index.contains(&format!("]({})\n", page.file)),
            "the index links {}",
            page.file
        );
    }
}

#[test]
fn write_manual_replaces_generated_files_and_keeps_unrelated_files() {
    let scratch = Scratch::new("replace");
    let out = scratch.path("out");
    fs::create_dir_all(&out).expect("the output directory can be created");
    fs::write(scratch.path("out").join("keep.txt"), "mine").expect("the unrelated file");
    fs::write(
        out.join("index.md"),
        format!("{GENERATED_MARKER}\nstale bytes"),
    )
    .expect("the stale generated file");
    write_manual(&out).expect("the manual writes");
    assert_eq!(
        fs::read_to_string(out.join("keep.txt")).expect("the unrelated file"),
        "mine",
        "an unrelated file survives the write"
    );
    let index = fs::read_to_string(out.join("index.md")).expect("the index");
    assert!(
        index.contains("# clio help index"),
        "the index was replaced"
    );
}

#[test]
fn write_manual_refuses_an_unrecognized_file_in_a_pages_place() {
    let scratch = Scratch::new("unrecognized");
    let out = scratch.path("out");
    fs::create_dir_all(&out).expect("the output directory can be created");
    fs::write(out.join("index.md"), "hand written").expect("the colliding file");
    let error = write_manual(&out).expect_err("a foreign file is refused");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        fs::read_to_string(out.join("index.md")).expect("the colliding file"),
        "hand written",
        "the refused file is unchanged"
    );
}

#[test]
fn write_manual_refuses_a_directory_in_a_pages_place() {
    let scratch = Scratch::new("directory");
    let out = scratch.path("out");
    fs::create_dir_all(out.join("index.md")).expect("the colliding directory");
    let error = write_manual(&out).expect_err("a directory is refused");
    assert_eq!(error.code(), "usage");
}

#[test]
fn a_refused_out_path_is_never_created() {
    let scratch = Scratch::new("fresh-refusal");
    let blocker = scratch.path("blocker");
    fs::write(&blocker, "not a directory").expect("the blocking file");
    // The walk refuses before anything is created, so the path below the file
    // stays absent. A page target cannot be inspected inside a directory that
    // does not exist, so on a fresh path the walk is the check that can fail;
    // it must leave nothing behind.
    let out = blocker.join("manual");
    let error = write_manual(&out).expect_err("a path through a file is refused");
    assert_eq!(error.code(), "usage");
    assert!(!out.exists(), "the refused output path was not created");
}

#[cfg(unix)]
#[test]
fn write_manual_refuses_a_symlink_page_target() {
    let scratch = Scratch::new("symlink-target");
    let out = scratch.path("out");
    fs::create_dir_all(&out).expect("the output directory can be created");
    let outside = scratch.path("outside.md");
    fs::write(&outside, "untouched").expect("the outside file");
    std::os::unix::fs::symlink(&outside, out.join("index.md")).expect("the symlink");
    let error = write_manual(&out).expect_err("a symlink target is refused");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        fs::read_to_string(&outside).expect("the outside file"),
        "untouched",
        "the symlink's target is unchanged"
    );
}

#[cfg(unix)]
#[test]
fn write_manual_refuses_a_symlink_directory_component() {
    let scratch = Scratch::new("symlink-dir");
    let real = scratch.path("real");
    fs::create_dir_all(&real).expect("the real directory");
    let link = scratch.path("link");
    std::os::unix::fs::symlink(&real, &link).expect("the symlink");
    let error = write_manual(&link).expect_err("a symlink component is refused");
    assert_eq!(error.code(), "usage");
}

#[cfg(unix)]
#[test]
fn write_manual_refuses_a_fifo_in_a_pages_place() {
    use std::os::unix::fs::FileTypeExt;
    let scratch = Scratch::new("fifo");
    let out = scratch.path("out");
    fs::create_dir_all(&out).expect("the output directory can be created");
    let fifo = out.join("index.md");
    let created = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo runs");
    assert!(created.success(), "mkfifo created the fifo: {created}");
    // Opening the fifo for reading would block until a writer appears, so the
    // refusal must come before any open. The timeout is a safety net for a
    // regression that opens it: the test then fails instead of hanging.
    let (sender, receiver) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let _ = sender.send(write_manual(&out).map_err(|error| error.code().to_owned()));
    });
    let outcome = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the fifo is refused without opening it");
    writer.join().expect("the writer thread finished");
    assert_eq!(outcome, Err("usage".to_owned()), "the fifo is a refusal");
    assert!(
        fs::symlink_metadata(&fifo)
            .expect("the fifo")
            .file_type()
            .is_fifo(),
        "the refused fifo is left untouched"
    );
}

#[cfg(unix)]
#[test]
fn write_manual_rechecks_the_targets_after_creating_the_output() {
    let scratch = Scratch::new("late-resolution");
    fs::write(scratch.path("index.md"), "hand written").expect("the unrelated file");
    // Creating `missing` changes how `missing/..` resolves: only after the
    // creation do the targets and the writes both name the scratch root, so
    // the check has to run once more before the first write can replace the
    // unrelated file the pre-creation check could not see.
    let out = scratch.path("missing").join("..");
    let error = write_manual(&out).expect_err("the unrelated file is refused");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        fs::read_to_string(scratch.path("index.md")).expect("the unrelated file"),
        "hand written",
        "the refused file is unchanged"
    );
}

#[test]
fn a_page_name_that_leaves_the_output_directory_is_refused() {
    let scratch = Scratch::new("traversal");
    let out = scratch.path("out");
    fs::create_dir_all(&out).expect("the output directory");
    for name in ["../escape.md", "sub/page.md", "..", "."] {
        let error = check_target(&out, name).expect_err("a traversing name is refused");
        assert_eq!(error.code(), "usage", "name {name:?}");
    }
}

#[test]
fn a_write_step_failure_reports_an_incomplete_manual() {
    let error = incomplete(Path::new("/tmp/manual"), &CliError::usage("boom"));
    assert_eq!(error.code(), "manual_incomplete");
    assert!(error.message().contains("boom"), "the step is named");
    assert!(
        error.message().contains("incomplete"),
        "the partial outcome is named"
    );
}

/// Arm the promote seam for one target until the guard drops, so a panicking
/// test cannot leave the seam armed for the next one.
struct ArmedPromoteFailure;

impl ArmedPromoteFailure {
    fn arm(target: &Path) -> Self {
        *FAILED_PROMOTE_TARGET
            .lock()
            .expect("the promote seam lock is not poisoned") = Some(target.to_path_buf());
        Self
    }
}

impl Drop for ArmedPromoteFailure {
    fn drop(&mut self) {
        *FAILED_PROMOTE_TARGET
            .lock()
            .expect("the promote seam lock is not poisoned") = None;
    }
}

#[test]
fn a_failed_promote_reports_the_incomplete_manual_and_leaves_no_temp_sibling() {
    let scratch = Scratch::new("promote-failure");
    let out = scratch.path("out");
    fs::create_dir_all(&out).expect("the output directory can be created");
    let unrelated = out.join("keep.txt");
    let unrelated_bytes = b"unrelated bytes, not a generated manual\n";
    fs::write(&unrelated, unrelated_bytes).expect("the unrelated file");

    let manual = render_manual();
    let mut order = manual.write_order();
    let first = order.next().expect("a first page");
    let second = order.next().expect("a second page");
    let _armed = ArmedPromoteFailure::arm(&out.join(&second.file));

    let error = write_manual(&out).expect_err("the second promote step fails");

    assert_eq!(error.code(), "manual_incomplete");
    let message = error.message();
    assert!(
        message.contains("incomplete"),
        "the partial outcome: {message}"
    );
    assert!(message.contains("promote"), "the failed step: {message}");
    assert!(
        message.contains(&second.file),
        "the failed target: {message}"
    );

    assert_eq!(
        fs::read(out.join(&first.file)).expect("the first page"),
        first.bytes.as_bytes(),
        "the already-promoted page keeps its real bytes"
    );
    assert_eq!(
        fs::read(&unrelated).expect("the unrelated file"),
        unrelated_bytes,
        "the unrelated file is byte-identical"
    );

    let mut names: Vec<String> = fs::read_dir(&out)
        .expect("the output directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort_unstable();
    assert!(
        !names.iter().any(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
        }),
        "the failed step's temp sibling was removed: {names:?}"
    );
    let mut expected = vec![first.file.clone(), "keep.txt".to_owned()];
    expected.sort_unstable();
    assert_eq!(
        names, expected,
        "only the promoted page and the unrelated file remain"
    );
}

#[cfg(unix)]
#[test]
fn a_replaced_page_keeps_its_mode_and_a_new_page_is_0644() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("mode");
    let out = scratch.path("out");
    fs::create_dir_all(&out).expect("the output directory");
    let index = out.join("index.md");
    fs::write(&index, format!("{GENERATED_MARKER}\nstale")).expect("the stale file");
    fs::set_permissions(&index, fs::Permissions::from_mode(0o600)).expect("the mode can be set");
    write_manual(&out).expect("the manual writes");
    let replaced = fs::symlink_metadata(&index).expect("the replaced file");
    assert_eq!(replaced.permissions().mode() & 0o777, 0o600, "mode kept");
    let fresh = fs::symlink_metadata(out.join("get.md")).expect("a new page");
    assert_eq!(fresh.permissions().mode() & 0o777, 0o644, "new page mode");
}
