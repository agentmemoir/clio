// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the options block rendered from a command
//! record: the per-flag help lines, and the documented-versus-extra grouping
//! every per-command help path prints.
//!
//! ## Owns
//! - `options_help`, the block the usage lookups append to a record's text
//! - `flag_help_line` and `help_value`, one flag's line and its value token
//!
//! ## Does not own
//! - The record model and its fields (see `cli_command_spec`)
//! - The usage line and its alternates (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//!
//! ## Boundary
//!
//! Changes to how a flag's help line is written should normally require
//! changes here. Changes to one command's flags should not.
//!
//! Keep this module focused on the options block's own rendering.

use super::{CommandSpec, Flag};

impl CommandSpec {
    /// The options block every per-command help path appends: the record's own
    /// flags under `Options:`, then the globals it accepts without documenting
    /// under `Global flags:`, each on its own line.
    ///
    /// The two groups stay separate because they mean different things: an
    /// `Options:` flag is this command's own grammar, while a `Global flags:`
    /// entry is a declared global the record accepts but never prints, rendered
    /// from its shared `GLOBAL_*` definition so every command describes it the
    /// same way. A block with nothing to show prints nothing, so a heading
    /// never stands over an empty list. A flag line identical to one already
    /// shown is skipped, so a flag documented twice prints once.
    ///
    /// # Panics
    /// Panics when `accepted_extra` names something that is not a declared
    /// global flag, the same fail-closed rule `parser_spec` applies: an
    /// unvalidated record must not render a flag the registry never declared.
    #[must_use]
    pub(crate) fn options_help(&self) -> String {
        let mut block = String::new();
        let mut shown: Vec<String> = Vec::new();
        if !self.documented.is_empty() {
            block.push_str("Options:");
            for flag in self.documented {
                let line = Self::flag_help_line(flag);
                if shown.contains(&line) {
                    continue;
                }
                shown.push(line.clone());
                block.push('\n');
                block.push_str(&line);
            }
        }
        if !self.accepted_extra.is_empty() {
            let mut extras = String::new();
            for name in self.accepted_extra {
                let flag = crate::cli_command_registry::global_flag(name)
                    .expect("accepted_extra names a declared global flag");
                let line = Self::flag_help_line(flag);
                if shown.contains(&line) {
                    continue;
                }
                shown.push(line.clone());
                extras.push('\n');
                extras.push_str(&line);
            }
            if !extras.is_empty() {
                if !block.is_empty() {
                    block.push('\n');
                }
                block.push_str("Global flags:");
                block.push_str(&extras);
            }
        }
        block
    }

    /// One flag's help line: `  --long VALUE  description [default: X]`.
    ///
    /// The value token follows the flag when it takes a value, and the
    /// description and `[default: X]` suffix print only when the record carries
    /// them, so a blank field never prints as an empty gap. A declared short
    /// spelling prints nowhere here: it stays documented once, in the top-level
    /// help, the way the usage lines already keep it.
    fn flag_help_line(flag: &Flag) -> String {
        let mut line = format!("  --{}", flag.long);
        if let Some(value) = flag.help_value() {
            line.push(' ');
            line.push_str(&value);
        }
        if !flag.description.is_empty() {
            line.push_str("  ");
            line.push_str(flag.description);
        }
        if let Some(default) = flag.default {
            use std::fmt::Write as _;
            let _ = write!(line, " [default: {default}]");
        }
        line
    }
}

impl Flag {
    /// The value token the flag's help line prints: the placeholder override
    /// when one is set, else the value's placeholder, bracketed when the value
    /// may be omitted.
    fn help_value(&self) -> Option<String> {
        let value = self.value?;
        let placeholder = self.placeholder_override.unwrap_or(value.placeholder);
        Some(if self.value_optional {
            format!("[{placeholder}]")
        } else {
            placeholder.to_owned()
        })
    }
}

#[cfg(test)]
#[path = "cli_command_spec_options_tests.rs"]
mod tests;
