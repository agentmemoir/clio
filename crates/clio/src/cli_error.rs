// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared CLI error contract: one error type
//! carrying a machine code, a human message, an optional hint, an optional raw
//! help block, and its exit code.
//!
//! ## Owns
//! - The `CliError` type and its usage/operational constructors
//! - The `message` / `hint` / `help` split (JSON keeps them separate; text mode
//!   prints `message`, the raw `help` block, then a `hint:` line)
//! - Mapping in-process failures into the CLI contract (credentials masked, then
//!   the input names translated by `cli_error_names`)
//! - The 0/1/2 exit-code mapping shared by the read commands
//!
//! ## Does not own
//! - Output-mode selection or rendering (see `cli_output`)
//! - Argument parsing (see `cli_args`) or read-command semantics (see `cli_read`)
//! - The internal-name to flag-name vocabulary (see `cli_error_names`)
//!
//! ## Boundary
//!
//! Changes to the CLI error shape or exit-code mapping should normally require changes here.
//! Changes to an in-process tool's failure semantics should not require changes here.
//!
//! Keep this module focused on the shared CLI error contract.

use clio_types::AmError;
use serde_json::{Value, json};

/// Process exit code for a successful command.
pub(crate) const EXIT_OK: u8 = 0;
/// Process exit code for an operational failure (a read failed, a store could not open).
pub(crate) const EXIT_OPERATIONAL: u8 = 1;
/// Process exit code for a usage error (bad flag, missing argument, unknown command).
pub(crate) const EXIT_USAGE: u8 = 2;
/// Process exit code for a restore that replaced the database but not the key
/// store. Distinct from both success and a refusal: the operator must be able
/// to tell a partial restore from either.
pub(crate) const EXIT_PARTIAL: u8 = 3;

/// One CLI failure: a machine code, a human message, an optional hint, an
/// optional raw help block, and the process exit code it maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliError {
    code: String,
    message: String,
    hint: Option<String>,
    help: Option<String>,
    exit: u8,
}

impl CliError {
    /// A usage error (exit 2): a bad flag, a missing argument, or a bad value.
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self {
            code: "usage".to_owned(),
            message: message.into(),
            hint: None,
            help: None,
            exit: EXIT_USAGE,
        }
    }

    /// A refusal (exit 2) carrying the failing subsystem's own code.
    ///
    /// A refused command is a usage outcome, not a runtime failure: the input
    /// was understood and deliberately not acted on. The code is kept so a
    /// script can tell a corrupt archive from a backend mismatch without
    /// parsing the message. The message goes through the credential redactor
    /// here, so a constructor that takes a foreign error string cannot skip
    /// the redaction the security condition requires.
    pub(crate) fn refusal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: redact(&message.into()),
            hint: None,
            help: None,
            exit: EXIT_USAGE,
        }
    }

    /// A usage error with an actionable hint (exit 2).
    pub(crate) fn usage_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::usage(message).with_hint(hint)
    }

    /// A usage error with a raw multi-line help block (exit 2).
    ///
    /// Text mode prints the message, a blank line, then the block verbatim (no
    /// `hint:` label). JSON mode carries the block in a separate `help` field so
    /// `message` stays a single actionable line.
    pub(crate) fn usage_help(message: impl Into<String>, help: impl Into<String>) -> Self {
        Self::usage(message).with_help(help)
    }

    /// An operational failure (exit 1) with an explicit machine code.
    pub(crate) fn operational(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: None,
            help: None,
            exit: EXIT_OPERATIONAL,
        }
    }

    /// Map an in-process `AmError` into the CLI contract, masking credentials in
    /// the message so a driver error can never leak a connection password.
    pub(crate) fn from_am_error(error: &AmError) -> Self {
        Self::operational(error.code.as_str(), name_cli_inputs(&error.message))
    }

    /// An operational failure built from an in-process/transport payload code
    /// and message, with credentials masked in the message.
    pub(crate) fn tool_failure(code: &str, message: &str) -> Self {
        Self::operational(code, name_cli_inputs(message))
    }

    /// Attach an actionable hint.
    pub(crate) fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// This failure re-mapped to another exit code, with the message put
    /// through the redactor again so a re-mapped failure cannot skip the
    /// redaction its constructor applied.
    ///
    /// A command whose failures are mapped from one condition-to-code table
    /// builds the error with a generic constructor and sets the code here, so
    /// no call site chooses an exit code for itself.
    pub(crate) fn with_exit(mut self, exit: u8) -> Self {
        self.message = redact(&self.message);
        self.exit = exit;
        self
    }

    /// Attach a raw multi-line help block.
    pub(crate) fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Machine-readable code (`usage`, or the in-process `ErrorCode`).
    #[cfg(test)]
    pub(crate) fn code(&self) -> &str {
        &self.code
    }

    /// Human-readable message.
    ///
    /// Not test-only: a surface that reports its own failure as a plain string
    /// rather than as an envelope needs the text without the formatting.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// Optional actionable hint.
    #[cfg(test)]
    pub(crate) fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Optional raw help block.
    #[cfg(test)]
    pub(crate) fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }

    /// The process exit code this failure maps to.
    pub(crate) fn exit_code(&self) -> std::process::ExitCode {
        std::process::ExitCode::from(self.exit)
    }

    /// The structured `{ok:false, code, message, hint?, help?}` envelope.
    pub(crate) fn to_envelope(&self) -> Value {
        let mut envelope = json!({
            "ok": false,
            "code": self.code,
            "message": self.message,
        });
        if let Some(hint) = &self.hint {
            envelope["hint"] = Value::String(hint.clone());
        }
        if let Some(help) = &self.help {
            envelope["help"] = Value::String(help.clone());
        }
        envelope
    }

    /// Human-readable rendering: the message, an optional raw help block, and an
    /// optional `hint:` line.
    pub(crate) fn to_human(&self) -> String {
        let mut text = format!("error: {}", self.message);
        if let Some(help) = &self.help {
            text.push_str("\n\n");
            text.push_str(help);
        }
        if let Some(hint) = &self.hint {
            text.push_str("\nhint: ");
            text.push_str(hint);
        }
        text
    }
}

/// Reuse the ops credential redactor so every CLI error string is scrubbed.
fn redact(message: &str) -> String {
    clio_ops::redact_credentials(message)
}

/// Attach a pointer to `clio <command> --help` to a usage failure with no hint.
///
/// One place, applied at each surface's parse boundary, so an unknown flag with
/// no near miss still says where the list of real flags is. A failure that
/// already carries a hint keeps it: a suggestion beats a pointer.
pub(crate) fn with_help_hint(error: CliError, command: &str) -> CliError {
    if error.hint.is_some() || error.code != "usage" {
        return error;
    }
    error.with_hint(format!("try `clio {command} --help`"))
}

/// The CLI spelling of an input name the library uses, for a CLI-facing message.
///
/// Only the input vocabulary is translated. Everything else in a library
/// message — its wording, its bounds, its reasoning — reaches the caller intact,
/// because that part is accurate and useful.
pub(crate) fn name_cli_inputs(message: &str) -> String {
    crate::cli_error_names::to_cli_spelling(&clio_ops::redact_credentials(message))
}

/// Refuse an empty value for a flag the caller supplied explicitly.
///
/// One rule for every value flag, on every surface, so a caller can predict it.
/// The flag is named in both the message and the hint, and the hint says what to
/// do instead.
///
/// It lives here rather than beside either parser because there are two of them:
/// the strict grammar that the read and write commands share, and the name-to-
/// value map the reserved surfaces read. When the rule sat only in the first, a
/// command like `clio ops diagnose --db ''` silently used the default database
/// and no message anywhere said that was different from a refusal.
pub(crate) fn reject_empty_value(flag: &str, value: &str) -> Result<(), CliError> {
    if !value.is_empty() {
        return Ok(());
    }
    Err(CliError::usage_hint(
        format!("{flag} must not be empty when supplied (omit the flag instead)"),
        format!("pass a non-empty value or omit {flag}"),
    ))
}

/// The actionable hint for a cross-process key failure, if `message` is one.
///
/// The default key provider is process-local, so content written by one
/// one-shot CLI process cannot be decrypted by the next. Surface that at the
/// point of failure instead of leaving a bare `forbidden`.
pub(crate) fn cross_process_dek_hint(message: &str) -> Option<&'static str> {
    message.contains("no DEK for subject").then_some(
        "the default key provider is process-local; content written by another process needs a durable KMS provider",
    )
}

#[cfg(test)]
#[path = "cli_error_tests.rs"]
mod tests;
