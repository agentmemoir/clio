// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the process's own `SIGPIPE` disposition: the
//! default that ends a one-shot command quietly, and the ignore a long-running
//! transport re-installs so one client going away cannot take the server down.
//!
//! ## Owns
//!
//! - `restore_default`, the `SIG_DFL` disposition for a short-lived command
//! - `ignore`, the `SIG_IGN` disposition for a long-running transport
//! - `begin_serving`, the witness a serve entry point requires before serving
//!
//! ## Does not own
//!
//! - What the process writes, or which surface it is (`main` and the surfaces)
//! - Any other signal; clio changes no disposition but this one
//!
//! ## Boundary
//!
//! Changes to clio's `SIGPIPE` policy should normally require changes here.
//! Changes to a surface's output should not require changes here.
//!
//! `SIGPIPE` is a Unix signal, so both functions are no-ops elsewhere.

/// Restore the default `SIGPIPE` disposition.
///
/// Rust ignores `SIGPIPE` at startup, so a write to a pipe whose reader has gone
/// returns `EPIPE` and the standard library's `println!` panics on it. `SIG_DFL`
/// restores the Unix default, where the kernel ends the process on the same
/// condition with no message and no panic, which is what a one-shot command in a
/// pipeline wants.
#[cfg(unix)]
pub(crate) fn restore_default() {
    #[allow(unsafe_code)]
    // SAFETY: setting a signal to `SIG_DFL` is async-signal-safe and touches no
    // memory this program owns; the only effect is the process's own disposition.
    unsafe {
        let _ = libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Ignore `SIGPIPE` again, so a write to a closed socket or pipe is a recoverable
/// `EPIPE` instead of a signal that ends the process.
///
/// A long-running transport must outlive one client. Without this, the default
/// restored at startup would let a single dropped connection kill the whole
/// server with no error and no log, and `SIGPIPE` sits in the "clean exit" set,
/// so a supervisor may not even restart it.
#[cfg(unix)]
pub(crate) fn ignore() {
    #[allow(unsafe_code)]
    // SAFETY: as for `restore_default`; `SIG_IGN` is async-signal-safe and
    // process-local.
    unsafe {
        let _ = libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// No-op off Unix: `SIGPIPE` is a Unix signal and the Windows console raises none.
#[cfg(not(unix))]
pub(crate) fn restore_default() {}

/// A witness that [`begin_serving`] ran for a long-running transport.
///
/// The field is private, so the only way to hold one is to call
/// [`begin_serving`]; a serve entry point that requires the witness cannot be
/// called without first installing the disposition.
pub(crate) struct Serving(());

impl Serving {
    /// The witness without touching the process disposition, for the unit tests
    /// that drive a serve entry point without exercising signal handling.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self(())
    }
}

/// Ignore `SIGPIPE` and return the witness a serve entry point requires.
///
/// The witness is what keeps the disposition and the serving tied together: a
/// future edit can drop a `sigpipe::ignore()` line, but it cannot drop a
/// parameter the compiler demands.
pub(crate) fn begin_serving() -> Serving {
    ignore();
    Serving(())
}

/// No-op off Unix, for the same reason as [`restore_default`].
#[cfg(not(unix))]
pub(crate) fn ignore() {}

#[cfg(test)]
#[path = "sigpipe_tests.rs"]
mod tests;
