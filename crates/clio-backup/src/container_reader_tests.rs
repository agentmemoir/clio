// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the reader's total-byte budget at
//! the point it is enforced: on the total a chunk would produce, before that
//! chunk reaches the caller's writer.
//!
//! ## Owns
//! - The pre-write total-budget refusal and the bytes it leaves unwritten
//!
//! ## Does not own
//! - Truncation, header, and end-marker refusals (see `container_corrupt_tests`)
//! - The writer side (see `container_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the total-budget check moves or changes its order
//! against the write.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the reader's total-byte budget.

use super::{ArchiveReader, ReaderBounds};
use crate::fixtures::Scratch;
use crate::tar_header::{HEADER_SIZE, build_header};
use clio_types::ErrorCode;
use std::fs::File;
use std::io::Write;

/// The manifest body these tests carry: the reader bounds a member, it does
/// not parse what the member holds.
const MANIFEST_BODY: &[u8] = b"{}";

/// The payload member's name and length, chosen so the total budget is crossed
/// well after writes have begun.
const PAYLOAD_MEMBER: &str = "store.db";
const PAYLOAD_LEN: usize = 100_000;

/// Bytes owed past a member's body so the next header block starts aligned.
fn padding(len: usize) -> usize {
    (HEADER_SIZE - len % HEADER_SIZE) % HEADER_SIZE
}

/// Append one ustar member the way the writer lays it out: header, body,
/// zero padding.
fn push_member(raw: &mut Vec<u8>, name: &str, body: &[u8]) {
    raw.extend_from_slice(&build_header(name, body.len() as u64).expect("member header"));
    raw.extend_from_slice(body);
    raw.extend(vec![0u8; padding(body.len())]);
}

/// A gzip archive of a manifest member plus one payload member, and the count
/// the manifest member contributes to the reader's running total.
fn archive_with_payload() -> (Vec<u8>, u64) {
    let mut raw = Vec::new();
    push_member(&mut raw, crate::MANIFEST_MEMBER, MANIFEST_BODY);
    let payload = vec![0x5Au8; PAYLOAD_LEN];
    push_member(&mut raw, PAYLOAD_MEMBER, &payload);
    raw.extend([0u8; 2 * HEADER_SIZE]);

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&raw).expect("gzip the archive");
    let gz = encoder.finish().expect("gzip trailer");
    let manifest_counted = (MANIFEST_BODY.len() + padding(MANIFEST_BODY.len())) as u64;
    (gz, manifest_counted)
}

/// Open the archive at `path` under `max_total_bytes` and leave the payload
/// member open, the manifest member having been skipped.
fn open_payload_member(path: &std::path::Path, max_total_bytes: u64) -> ArchiveReader<File> {
    let bounds = ReaderBounds {
        max_members: 1024,
        max_member_bytes: u64::MAX,
        max_total_bytes,
    };
    let mut reader =
        ArchiveReader::with_bounds(File::open(path).expect("open the archive"), bounds)
            .expect("the bounds are accepted");
    reader
        .next_member()
        .expect("manifest header")
        .expect("manifest member");
    reader.skip_current().expect("manifest payload");
    reader
        .next_member()
        .expect("payload header")
        .expect("payload member");
    reader
}

/// The total budget is tested against the total the next chunk would produce,
/// so a member refused for crossing it leaves nothing past the budget in the
/// sink it was staging to.
#[test]
fn total_budget_is_refused_on_the_projected_total_before_the_write() {
    let scratch = Scratch::new("reader-total-budget");
    let archive_path = scratch.join("archive.tar.gz");
    let out_path = scratch.join("staged-member.bin");

    let (gz, manifest_counted) = archive_with_payload();
    std::fs::write(&archive_path, &gz).expect("write the archive");

    // The budget is small enough that the payload cannot fit and large enough
    // that whole chunks are written before the refusal: a check made after the
    // write would leave the crossing chunk behind, and the staged length below
    // would pass its budget.
    let chunk = 8192;
    let budget = 3 * chunk + 100;
    let max_total_bytes = manifest_counted + budget;
    let mut reader = open_payload_member(&archive_path, max_total_bytes);

    let mut out = File::create(&out_path).expect("create the staged member");
    let err = reader
        .drain_current(Some(&mut out))
        .expect_err("the member crosses the total budget");
    assert_eq!(err.code, ErrorCode::ArchiveCorrupt, "{}", err.message);
    assert!(
        err.message.contains(&format!(
            "archive members exceed the total inflated-size cap of {max_total_bytes} bytes"
        )),
        "{}",
        err.message
    );

    let written = std::fs::metadata(&out_path)
        .expect("the staged member exists")
        .len();
    assert!(
        written > 0,
        "the budget left room for whole chunks, so the refusal must come after writes"
    );
    assert!(
        written <= budget,
        "{written} bytes reached the staged member under a budget of {budget}"
    );
}
