// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! One-time registration of the sqlite-vec auto-extension.

use crate::migrate::sqlite_vec_missing;
use clio_types::AmError;
use std::sync::OnceLock;

static REGISTER: OnceLock<Result<(), String>> = OnceLock::new();

/// Map a registration outcome to [`AmError`] (testable without failing process init).
pub(crate) fn map_register_outcome(outcome: &Result<(), String>) -> Result<(), AmError> {
    match outcome {
        Ok(()) => Ok(()),
        Err(detail) => Err(sqlite_vec_missing(detail)),
    }
}

/// Interpret `sqlite3_auto_extension` return codes (testable both arms).
pub(crate) fn interpret_auto_extension_rc(result: std::os::raw::c_int) -> Result<(), String> {
    if result == rusqlite::ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(format!("sqlite3_auto_extension returned {result}"))
    }
}

/// Register `sqlite3_vec_init` as a SQLite auto-extension (process-wide).
///
/// # Errors
/// When registration fails — callers must fail closed (no pretend vector mode).
pub fn ensure_sqlite_vec_registered() -> Result<(), AmError> {
    let outcome = REGISTER.get_or_init(|| {
        #[allow(unsafe_code, clippy::missing_transmute_annotations)]
        // SAFETY: transmute matches the C callback signature required by SQLite;
        // `sqlite3_vec_init` is the extension entrypoint from the sqlite-vec crate.
        let result = unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )))
        };
        interpret_auto_extension_rc(result)
    });
    map_register_outcome(outcome)
}
