// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the production masked reader: that
//! it exists, refuses cleanly without a terminal, and is what a `Ctx` gets before
//! a test replaces it.
//!
//! ## Owns
//! - Production-reader behavior under test for `setup_secrets`
//!
//! ## Does not own
//! - The scripted reader (see `setup_test_seam`)
//! - Any secret actually read in a test
//!
//! ## Boundary
//!
//! Changes to the production reader should normally update tests here.
//! Changes to the seam's shape should not.
//!
//! Keep this module focused on the production reader.

use super::*;
use std::io::Write;

/// A writer that discards, standing in for a terminal a test has no business
/// touching.
struct Discard;

impl Write for Discard {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn the_production_reader_is_the_rpassword_one() {
    // The test binary's stdin is not a terminal, so this refuses. What is being
    // asserted is that the production reader refuses through the seam rather than
    // panicking or hanging, which is the failure mode that would block CI.
    let mut out = Discard;
    let outcome = from_terminal(&mut out, "Password");
    assert!(outcome.is_err(), "a piped stdin must not produce a secret");
}

#[test]
fn the_production_reader_constructs() {
    let mut reader = production_reader();
    let mut out = Discard;
    assert!(reader(&mut out, "Password").is_err());
}
