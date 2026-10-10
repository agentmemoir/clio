// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one environment setting every integration
//! harness needs before it runs the real `clio` binary: where the Clio
//! encryption key lives.
//!
//! ## Owns
//! - Seeding one owner-only key file per test binary and pointing `CLIO_KEY_FILE`
//!   at it
//!
//! ## Does not own
//! - Any scenario, database, or key-store behaviour (each harness owns its own)
//! - The key's lifecycle rules (see `clio-config`'s `secret_key`)
//!
//! ## Boundary
//!
//! Keep this module to the key-file seed. If a harness needs more environment
//! than this, it should set it itself rather than widen this.
//!
//! # Why every harness needs this
//!
//! These harnesses clear the environment, so the binary has no home directory.
//! Clio now keeps one encryption key and seals the per-subject keys in the
//! key-store file under it, so a `remember` needs that key. Without this seed the
//! binary would fall through to the operating system's secret store, which on a
//! developer machine means writing a real entry into their login Keychain — and
//! then a second harness that prefers the file, or the same one on a later run,
//! meets a key in both stores and every command refuses to start.
//!
//! A key file is authoritative the moment it exists, so seeding it here means the
//! keystore is asked only whether a second key is also there, which it can answer
//! without writing anything. This is also the path CI takes.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The key every test in one binary seals under.
///
/// A fixed value, not a random one: nothing here asserts on a specific key, and
/// a fixed value makes a failure reproducible.
const TEST_KEY: [u8; 32] = [0x6b; 32];

/// The seeded key file, created once per test binary.
fn key_file() -> &'static PathBuf {
    static FILE: OnceLock<PathBuf> = OnceLock::new();
    FILE.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("clio-harness-key-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("harness key dir");
        let path = dir.join("clio.key");
        std::fs::write(&path, TEST_KEY).expect("seed the harness key file");
        path
    })
}

/// Point `cmd` at the seeded Clio encryption key, and away from the OS keystore.
///
/// Call this on every `Command` that runs the binary, after `env_clear` and
/// before spawning.
///
/// `CLIO_KEY_FILE_ONLY` is the half that matters most. A key file on its own is
/// not enough: the lifecycle still asks the keystore whether a second key is
/// there, and on macOS that is a Keychain read the operating system may answer
/// with a password prompt. These harnesses start hundreds of processes, so
/// without the switch one test run asks the operator for their Keychain password
/// hundreds of times. With it, nothing here touches the Keychain at all.
pub fn seed_key(cmd: &mut Command) -> &mut Command {
    cmd.env("CLIO_KEY_FILE", key_file())
        .env("CLIO_KEY_FILE_ONLY", "1")
}
