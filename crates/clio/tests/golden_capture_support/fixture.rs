// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the golden fixture's on-disk shape: the capture record,
//! the fixture envelope, its path, the loader, and the compact atomic writer.
//! No tests live here.
//!
//! ## Owns
//! - The fixture records, their path, the loader, and the atomic writer
//!
//! ## Does not own
//! - The capture runner that fills the records (the parent module's `capture`)
//! - Any assertion (`golden_capture_test`, `golden_repeatability_test`)
//!
//! ## Boundary
//!
//! Changes to the fixture file's shape should normally require changes here.
//! Changes to how captures are taken or compared should not.
//!
//! Keep this module focused on the fixture's file shape.

// Capture-mode items run in the capture tool, not in a test binary.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The fixture schema version the capture tool writes.
pub const FIXTURE_VERSION: u32 = 1;

/// One captured command: its exact argument vector, exit code (absent when a
/// signal killed the child), and both output streams as exact UTF-8 text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureEntry {
    pub argv: Vec<String>,
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// The public golden fixture: one entry per command, in stable catalog order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoldenFixture {
    pub version: u32,
    pub entries: Vec<CaptureEntry>,
}

/// The public fixture's path, inside this crate's own `tests` directory.
#[must_use]
pub fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden_cli_help.json")
}

/// Load a golden fixture from disk.
pub fn load_fixture(path: &Path) -> std::io::Result<GoldenFixture> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Write a golden fixture atomically: a temp file in the target's directory,
/// then one rename over the target. Only the capture mode calls this.
pub fn write_fixture(fixture: &GoldenFixture, path: &Path) -> std::io::Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let bytes = compact_fixture_bytes(fixture)?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = directory.join(format!(".clio-golden-fixture-{pid}-{seq}.tmp"));
    let written = fs::File::create(&temp).and_then(|mut file| file.write_all(&bytes));
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    fs::rename(&temp, path)
}

/// Serialize compactly: a `version` line, then exactly one JSON object line
/// per entry, so the committed fixture stays small and line-diffable, while
/// `serde_json` still escapes every string byte-exactly.
fn compact_fixture_bytes(fixture: &GoldenFixture) -> std::io::Result<Vec<u8>> {
    let mut text = format!(
        "{{\n  \"version\": {},\n  \"entries\": [\n",
        fixture.version
    );
    let last = fixture.entries.len().saturating_sub(1);
    for (position, entry) in fixture.entries.iter().enumerate() {
        let line = serde_json::to_string(entry)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        text.push_str(&line);
        if position != last {
            text.push(',');
        }
        text.push('\n');
    }
    text.push_str("  ]\n}\n");
    Ok(text.into_bytes())
}
