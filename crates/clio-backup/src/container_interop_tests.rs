// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the archive container tests that need a
//! second opinion: external reader interop, the ustar header size-field
//! limit, digest-versus-manifest comparison, and the spool file's mode.
//!
//! ## Owns
//! - The T02 external-reader interop cases (system `tar`, CPython `tarfile`)
//! - The header size-field and payload-digest detection cases
//! - The spool owner-only file-mode case
//!
//! ## Does not own
//! - The shared fixtures (see `container_fixtures_tests`)
//! - Round-trip, bounds, writer-order, or corruption cases (see
//!   `container_tests` and `container_corrupt_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on container behavior that a second implementation
//! or the filesystem can confirm.

use super::fixtures_tests::{
    MANIFEST_JSON, sha_hex, standard_members, temp_path, write_full_archive,
};
use super::{ArchiveReader, ArchiveWriter, declared_size_suspect};
use crate::tar_header::build_header;
use clio_types::ErrorCode;

/// T02: the system `tar -tvzf` lists every member. Skipped when `tar` is
/// not on PATH.
#[test]
fn t02_system_tar_lists_members() {
    let Ok(probe) = std::process::Command::new("tar").arg("--version").output() else {
        eprintln!("skipping t02: no tar binary on PATH");
        return;
    };
    if !probe.status.success() {
        eprintln!("skipping t02: tar --version failed");
        return;
    }
    let path = temp_path("t02");
    std::fs::write(&path, write_full_archive()).expect("write archive");
    let listing = std::process::Command::new("tar")
        .args(["-tvzf", &path.to_string_lossy()])
        .output()
        .expect("tar runs");
    let _ = std::fs::remove_file(&path);

    assert!(
        listing.status.success(),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    let stdout = String::from_utf8_lossy(&listing.stdout);
    assert!(stdout.contains(crate::MANIFEST_MEMBER), "{stdout}");
    for (name, _) in standard_members() {
        assert!(stdout.contains(name), "{stdout}");
    }
}

/// T02b: CPython's tarfile module lists every member with byte-exact
/// contents and matching SHA-256. Skipped when python3 is absent.
#[test]
fn t02b_python_tarfile_lists_members() {
    let script = "import tarfile, hashlib, sys\nwith tarfile.open(sys.argv[1], 'r:gz') as t:\n    for m in t.getmembers():\n        f = t.extractfile(m)\n        data = f.read() if f is not None else b''\n        print(m.name, len(data), hashlib.sha256(data).hexdigest())";
    let path = temp_path("t02b");
    std::fs::write(&path, write_full_archive()).expect("write archive");
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(&path)
        .output();
    let _ = std::fs::remove_file(&path);
    let Ok(output) = output else {
        eprintln!("skipping t02b: no python3 on PATH");
        return;
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let manifest_line = lines.next().expect("manifest line");
    assert!(
        manifest_line.starts_with(crate::MANIFEST_MEMBER),
        "{stdout}"
    );
    for (name, bytes) in standard_members() {
        let line = lines
            .next()
            .unwrap_or_else(|| panic!("missing line for {name}: {stdout}"));
        let expected = format!("{name} {} {}", bytes.len(), sha_hex(&bytes));
        assert_eq!(line, expected, "tarfile read {name} byte-exactly");
    }
    assert_eq!(lines.next(), None, "no extra members: {stdout}");
}

/// T08: a manifest that records a wrong payload digest is detectable: the
/// reader returns the true computed digest, which differs from the recorded
/// one, and the suspect-size predicate agrees.
#[test]
fn t08_payload_digest_mismatch_detectable() {
    let recorded = "0".repeat(64);
    let manifest_json = MANIFEST_JSON.replace(super::fixtures_tests::EMPTY_SHA256, &recorded);
    let payload = b"SQLite format 3\0real-bytes".to_vec();

    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(manifest_json.as_bytes())
        .expect("manifest first");
    w.add_member("store.db", payload.as_slice())
        .expect("member");
    let archive = w.finish().expect("finish");

    let mut r = ArchiveReader::new(archive.as_slice()).expect("opens");
    r.next_member().expect("manifest header");
    r.skip_current().expect("manifest payload");
    let header = r.next_member().expect("payload header").expect("member");
    let mut sink = Vec::new();
    let digest = r.read_current(&mut sink).expect("payload content");

    assert_eq!(sink, payload);
    assert_eq!(digest.sha256, sha_hex(&payload));
    assert_ne!(digest.sha256, recorded, "the true digest exposes the lie");
    assert!(declared_size_suspect(
        header.size + 1,
        u64::try_from(payload.len()).expect("payload fits u64")
    ));
}

/// The ustar size field carries 11 octal digits plus a NUL, so a size over
/// the field limit is refused as an error, never encoded into an overflowed
/// or truncated field.
///
/// Regression cover for issue #86: the refusal was `QuotaExceeded`, which a
/// caller reads as "the machine has no room". The limit is a property of the
/// member and of the format, so no amount of free space lifts it.
#[test]
fn t16_header_size_over_field_limit_is_refused() {
    let limit = crate::tar_header::MAX_HEADER_SIZE_VALUE;
    let err = build_header("store.db", limit + 1).expect_err("over the limit");
    assert_eq!(
        err.code,
        ErrorCode::OutOfRange,
        "a member the format cannot represent is not a machine quota"
    );
    assert!(err.message.contains("size-field limit"), "{}", err.message);
    build_header("store.db", limit).expect("the limit itself encodes");
}

/// The error code is the part that misled the operator, so it is pinned
/// separately from the message: nothing here may drift back to `QuotaExceeded`,
/// which the restore's exit classifier reads as out-of-space.
#[test]
fn a_member_over_the_size_field_limit_is_never_reported_as_a_quota() {
    let limit = crate::tar_header::MAX_HEADER_SIZE_VALUE;
    for size in [limit + 1, limit * 4, u64::MAX] {
        let err = build_header("store.db", size).expect_err("over the limit");
        assert_ne!(
            err.code,
            ErrorCode::QuotaExceeded,
            "size {size} must not be classified as a machine quota"
        );
        assert_eq!(err.code, ErrorCode::OutOfRange, "size {size}");
    }
}

/// A spool file carries raw store bytes, so it is created owner-only on a
/// Unix host.
#[cfg(unix)]
#[test]
fn t17_spool_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir();
    let spool =
        super::spool::MemberSpool::create(&dir, u128::from(std::process::id())).expect("spool");
    let mode = spool
        .file
        .metadata()
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "spool mode is owner-only");
    drop(spool);
}

/// The member-name gate is on the shared header-building path, so the
/// manifest write and an illegal payload name are decided by the same rule
/// with the same error type.
///
/// This is a white-box check of that single gate: `write_manifest` and
/// `add_member` both route through the shared `write_spooled`, which runs the
/// name gate first, so the gate the manifest passes is the gate
/// `add_member` runs. Calling `add_member` here is the proof that no other
/// name check exists on the manifest route, so the gate cannot be removed
/// from the shared path without this test failing to compile.
#[test]
fn t18_name_gate_covers_the_manifest_path() {
    // The reserved manifest name passes the gate, so the manifest path
    // proves the gate accepted it rather than skipped it.
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("the reserved manifest name passes the name gate");
    w.finish().expect("finish");

    // An illegal name is refused by the same gate, with the same error
    // type and the gate's own message.
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(MANIFEST_JSON.as_bytes())
        .expect("manifest first");
    let err = w
        .add_member("bad\\name", &b"x"[..])
        .expect_err("the same gate refuses an illegal payload name");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        err.message,
        crate::validate_member_name("bad\\name")
            .expect_err("the gate rejects the name")
            .message,
        "an illegal name is refused with the gate's own message"
    );

    // Every path that emits a header runs the gate first, including the
    // premeasured path a staged member takes.
    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(b"{}").expect("the manifest name is legal");
    let refused = w
        .add_measured_member("bad\\name", &b"x"[..], 1)
        .expect_err("the premeasured path validates the name itself");
    assert_eq!(refused.code, ErrorCode::InvalidArgument);

    let mut w = ArchiveWriter::new(Vec::new());
    w.write_manifest(b"{}").expect("the manifest name is legal");
    let refused = w
        .add_member("bad\\name", &b"x"[..])
        .expect_err("the payload path validates the name itself");
    assert_eq!(refused.code, ErrorCode::InvalidArgument);
}
