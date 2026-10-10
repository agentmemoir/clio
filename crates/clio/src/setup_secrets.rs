// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for every secret `clio setup` reads from the
//! operator: the typed database password and every hosted API key.
//!
//! ## Owns
//! - The masked-reader seam type, one signature for all secrets
//! - The production reader, which is the tree's only raw-terminal code
//! - Turning an unavailable terminal into a clear refusal
//!
//! ## Does not own
//! - The question that asks for a secret (see `setup_render::ask_masked`)
//! - What the secret is stored in or sealed with (see `setup_config`)
//! - Displaying a stored secret (see `setup_preview`)
//!
//! ## Boundary
//!
//! Changes to how a secret is read from a terminal should normally require
//! changes here. Changes to which questions ask for a secret should not.
//!
//! Keep this module focused on reading one secret without echoing it.
//!
//! # Why `rpassword` and nothing hand-rolled
//!
//! Turning echo off is terminal ioctl work, and the workspace denies `unsafe`,
//! so it cannot be written here. `rpassword` is one small dependency and the
//! whole of the tree's raw-terminal surface, which is a better trade than a
//! password typed in the clear landing in scrollback and in every pasted
//! terminal transcript.
//!
//! # Why the prompt is not written by `rpassword`
//!
//! `rpassword::prompt_password` writes its own prompt to the terminal rather
//! than to the injected `out` stream. That would make the wizard's output
//! untestable and would send styled text to a place nothing else writes to, so
//! the prompt goes to `out` here and only the read comes from `rpassword`.

use std::io::Write;

/// Reads one secret, given the prompt that was already written.
///
/// The writer is passed through so a caller can echo the newline the terminal
/// does not, and so the seam has one shape whether the reader is a terminal or a
/// scripted value.
pub(crate) type SecretReader = dyn FnMut(&mut dyn Write, &str) -> Result<String, String>;

/// Read one secret from the terminal, masked.
///
/// # Errors
/// The terminal could not be read, which happens whenever stdin is not one.
pub(crate) fn from_terminal(out: &mut dyn Write, _label: &str) -> Result<String, String> {
    match rpassword::read_password() {
        Ok(value) => Ok(value),
        Err(e) => Err(format!("could not read a masked value: {e}")),
    }
    .inspect_err(|_| {
        // `out` is written to so a caller that reached here on a piped stdin
        // learns why, rather than seeing a bare error with no prompt above it.
        let _ = writeln!(out);
    })
}

/// The production reader: `rpassword` behind the seam.
pub(crate) fn production_reader() -> Box<SecretReader> {
    Box::new(from_terminal)
}

#[cfg(test)]
#[path = "setup_secrets_tests.rs"]
mod tests;
