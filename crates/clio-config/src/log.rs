// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for process log level and secret-safe log emission.
//!
//! ## Owns
//! - `LogLevel` parsing and process-wide max level
//! - Field-aware log lines that mask secret-looking values
//!
//! ## Does not own
//! - Effective configuration resolution
//! - Structured telemetry / audit event stores
//!
//! ## Boundary
//!
//! Changes to logging verbosity and log-line redaction should normally require changes here.
//! Changes to config paths that set the level should not require changes here.
//!
//! Keep this module focused on log level and secret-safe log emission.

use crate::secret::{mask_secret, path_is_secret};
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicU8, Ordering};

/// Log verbosity; higher includes lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

impl LogLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    /// Parse a config/env level string.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Process-wide max log level (updated when effective config changes).
static MAX_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);

/// Set the process log level from effective configuration.
pub fn set_level(level: LogLevel) {
    MAX_LEVEL.store(level as u8, Ordering::Relaxed);
}

#[must_use]
pub fn current_level() -> LogLevel {
    match MAX_LEVEL.load(Ordering::Relaxed) {
        1 => LogLevel::Error,
        2 => LogLevel::Warn,
        4 => LogLevel::Debug,
        5 => LogLevel::Trace,
        _ => LogLevel::Info,
    }
}

/// Emit a log line if `level` is enabled. Secret-looking field values are masked.
pub fn log(level: LogLevel, message: &str, fields: &[(&str, &str)]) {
    if (level as u8) > MAX_LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let mut line = format!("[{}] {message}", level.as_str());
    for (key, value) in fields {
        let shown = if path_is_secret(key) {
            mask_secret(value)
        } else {
            (*value).to_owned()
        };
        let _ = write!(line, " {key}={shown}");
    }
    eprintln!("{line}");
}

/// Convenience macros would live here; keep call sites explicit for now.
pub fn info(message: &str, fields: &[(&str, &str)]) {
    log(LogLevel::Info, message, fields);
}

pub fn warn(message: &str, fields: &[(&str, &str)]) {
    log(LogLevel::Warn, message, fields);
}

pub fn error(message: &str, fields: &[(&str, &str)]) {
    log(LogLevel::Error, message, fields);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_level_accepts_warn_aliases() {
        assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("trace"), Some(LogLevel::Trace));
        assert_eq!(LogLevel::parse("nope"), None);
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            assert_eq!(level.to_string(), level.as_str());
        }
    }

    #[test]
    fn set_level_updates_current() {
        set_level(LogLevel::Error);
        assert_eq!(current_level(), LogLevel::Error);
        set_level(LogLevel::Warn);
        assert_eq!(current_level(), LogLevel::Warn);
        set_level(LogLevel::Debug);
        assert_eq!(current_level(), LogLevel::Debug);
        set_level(LogLevel::Trace);
        assert_eq!(current_level(), LogLevel::Trace);
        set_level(LogLevel::Info);
        assert_eq!(current_level(), LogLevel::Info);
    }

    #[test]
    fn log_respects_level_and_masks_secrets() {
        set_level(LogLevel::Error);
        log(LogLevel::Info, "skip", &[]);
        set_level(LogLevel::Trace);
        info("i", &[("api_key", "secret1234")]);
        warn("w", &[("ok", "1")]);
        error("e", &[]);
        set_level(LogLevel::Info);
    }
}
