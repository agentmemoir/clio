// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static command record model and the
//! usage-line rendering built from it.
//!
//! ## Owns
//! - `CommandSpec` and its parts: `Positional`, `Flag`, `ValueSpec`,
//!   `ValueKind`, `Example`, and `Section`
//! - `usage_line`, `full_usage`, `parser_spec`, and `required_flags`
//! - The options block every help path appends, rendered from the records by
//!   the `options` submodule beside this one
//!
//! ## Does not own
//! - The command records themselves (see `cli_command_registry_core_reads`)
//! - The registry that looks records up (see `cli_command_registry`)
//! - Argument parsing (see `cli_args`) or the help tables (see `cli_help_usage`)
//!
//! ## Boundary
//!
//! Changes to how a command's usage line is written should normally require
//! changes here. Changes to one command's flags should not.
//!
//! Keep this module focused on the record model and its rendering.
//

use crate::cli_args::FlagSpec;

/// One command's static record: its names, its parts, and where it belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    /// The command as typed, with no `clio ` prefix.
    pub(crate) name: &'static str,
    /// Other spellings that reach this command.
    pub(crate) aliases: &'static [&'static str],
    /// One catalog line.
    pub(crate) summary: &'static str,
    /// Prose lines printed under the usage line.
    pub(crate) notes: &'static [&'static str],
    /// Positional arguments, in print order.
    pub(crate) positionals: &'static [Positional],
    /// Flags the usage line prints, in print order.
    pub(crate) documented: &'static [Flag],
    /// Declared global flags this command accepts but never prints.
    pub(crate) accepted_extra: &'static [&'static str],
    /// Hidden spellings of this command's flags.
    pub(crate) flag_aliases: &'static [(&'static str, &'static str)],
    /// Replaces the whole assembled usage line when set.
    pub(crate) usage_override: Option<&'static str>,
    /// Extra usage lines, printed after the first one.
    pub(crate) usage_alternates: &'static [&'static str],
    /// Usage examples.
    pub(crate) examples: &'static [Example],
    /// The MCP tool this command calls, when it calls one.
    pub(crate) tool: Option<&'static str>,
    /// Where the catalog groups this command.
    pub(crate) section: Section,
}

/// One positional argument: its label and whether it must be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Positional {
    pub(crate) label: &'static str,
    pub(crate) required: bool,
    pub(crate) description: &'static str,
}

/// One command flag as the usage line writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Flag {
    pub(crate) long: &'static str,
    pub(crate) short: Option<char>,
    /// `None` makes this a boolean flag.
    pub(crate) value: Option<ValueSpec>,
    pub(crate) default: Option<&'static str>,
    pub(crate) required: bool,
    pub(crate) repeatable: bool,
    pub(crate) description: &'static str,
    pub(crate) placeholder_override: Option<&'static str>,
    /// The flag takes a value, but the value may be omitted.
    pub(crate) value_optional: bool,
    /// Consecutive flags sharing a group print as one `[--a|--b]` token.
    pub(crate) exclusive_group: Option<&'static str>,
}

/// A flag's value: the text the usage line prints and what kind of value it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValueSpec {
    pub(crate) placeholder: &'static str,
    pub(crate) kind: ValueKind,
}

/// The value kinds a flag can carry; documentation metadata only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    Text,
    #[cfg_attr(not(test), allow(dead_code))]
    Uint,
    #[cfg_attr(not(test), allow(dead_code))]
    Float,
    Enum(&'static [&'static str]),
}

/// One usage example.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Example {
    pub(crate) command: &'static str,
    pub(crate) comment: Option<&'static str>,
}

/// How a command affects stored data.
///
/// `Read` for read-only verbs, `Write` for verbs that create, modify, or
/// delete data, and `Reserved` for surfaces that do not consume it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Read,
    #[cfg_attr(not(test), allow(dead_code))]
    Write,
    #[cfg_attr(not(test), allow(dead_code))]
    Reserved,
}

impl Flag {
    /// A value-taking flag whose value must be given.
    pub(crate) const fn value(long: &'static str, placeholder: &'static str) -> Self {
        Self {
            long,
            short: None,
            value: Some(ValueSpec {
                placeholder,
                kind: ValueKind::Text,
            }),
            default: None,
            required: false,
            repeatable: false,
            description: "",
            placeholder_override: None,
            value_optional: false,
            exclusive_group: None,
        }
    }

    /// A flag that takes no value.
    pub(crate) const fn boolean(long: &'static str) -> Self {
        Self {
            long,
            short: None,
            value: None,
            default: None,
            required: false,
            repeatable: false,
            description: "",
            placeholder_override: None,
            value_optional: false,
            exclusive_group: None,
        }
    }

    /// A required-text value flag with its placeholder and one-line description.
    pub(crate) const fn text(
        long: &'static str,
        placeholder: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            description,
            ..Self::value(long, placeholder)
        }
    }

    /// A whole-number value flag with its placeholder and description.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn uint(
        long: &'static str,
        placeholder: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            description,
            value: Some(ValueSpec {
                placeholder,
                kind: ValueKind::Uint,
            }),
            ..Self::value(long, placeholder)
        }
    }

    /// A decimal value flag with its placeholder and description.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn float(
        long: &'static str,
        placeholder: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            description,
            value: Some(ValueSpec {
                placeholder,
                kind: ValueKind::Float,
            }),
            ..Self::value(long, placeholder)
        }
    }

    /// A value flag restricted to `allowed`, with its placeholder and description.
    pub(crate) const fn choice(
        long: &'static str,
        placeholder: &'static str,
        description: &'static str,
        allowed: &'static [&'static str],
    ) -> Self {
        Self {
            description,
            value: Some(ValueSpec {
                placeholder,
                kind: ValueKind::Enum(allowed),
            }),
            ..Self::value(long, placeholder)
        }
    }

    /// A boolean flag with its one-line description and no value.
    pub(crate) const fn toggle(long: &'static str, description: &'static str) -> Self {
        Self {
            description,
            ..Self::boolean(long)
        }
    }
}

impl CommandSpec {
    /// The usage line with no `usage: ` prefix and no prose.
    ///
    /// The line is `usage_override` when the record sets one; otherwise it is
    /// `clio <name>`, then each positional, then the documented flags.
    #[must_use]
    pub(crate) fn usage_line(&self) -> String {
        if let Some(line) = self.usage_override {
            return line.to_owned();
        }
        let name = self.name;
        let mut tokens: Vec<String> = vec![format!("clio {name}")];
        for positional in self.positionals {
            let label = positional.label;
            tokens.push(if positional.required {
                label.to_owned()
            } else {
                format!("[{label}]")
            });
        }
        let mut index = 0;
        while index < self.documented.len() {
            let flag = &self.documented[index];
            if let Some(group) = flag.exclusive_group {
                let mut run: Vec<&str> = Vec::new();
                while let Some(next) = self.documented.get(index)
                    && next.exclusive_group == Some(group)
                {
                    run.push(next.long);
                    index += 1;
                }
                let mut token = String::from("[");
                for (position, member) in run.iter().enumerate() {
                    if position > 0 {
                        token.push('|');
                    }
                    token.push_str("--");
                    token.push_str(member);
                }
                token.push(']');
                tokens.push(token);
                continue;
            }
            let long = flag.long;
            let mut token = format!("--{long}");
            if let Some(value) = &flag.value {
                let placeholder = value.placeholder;
                if flag.value_optional {
                    token.push_str(" [");
                    token.push_str(placeholder);
                    token.push(']');
                } else {
                    token.push(' ');
                    token.push_str(placeholder);
                }
            }
            if !flag.required {
                token = format!("[{token}]");
            }
            if flag.repeatable {
                token.push_str("...");
            }
            tokens.push(token);
            index += 1;
        }
        tokens.join(" ")
    }

    /// The full `--help` usage text: `usage: ` plus the line, then every
    /// alternate, then every note, each on a line indented by seven spaces.
    #[must_use]
    pub(crate) fn full_usage(&self) -> String {
        let mut text = format!("usage: {}", self.usage_line());
        for alternate in self.usage_alternates {
            text.push_str("\n       or: ");
            text.push_str(alternate);
        }
        for note in self.notes {
            text.push_str("\n       ");
            text.push_str(note);
        }
        text
    }

    /// The parser spec this record builds: documented flags plus the declared
    /// global flags in `accepted_extra`, with repeatable value flags marked.
    ///
    /// `kind`, `default`, `description`, `placeholder_override`, `short`,
    /// `section`, `examples`, `usage_override`, and `usage_alternates` are
    /// documentation metadata and never reach the parser.
    ///
    /// # Panics
    /// Panics when `accepted_extra` names something that is not a declared
    /// global flag. That is a static-data error, and the fail-closed panic
    /// keeps an unvalidated record from silently parsing a flag the usage line
    /// never mentions.
    #[must_use]
    pub(crate) fn parser_spec(&self) -> FlagSpec {
        let mut value: Vec<&'static str> = self
            .documented
            .iter()
            .filter(|flag| flag.value.is_some())
            .map(|flag| flag.long)
            .collect();
        let mut boolean: Vec<&'static str> = self
            .documented
            .iter()
            .filter(|flag| flag.value.is_none())
            .map(|flag| flag.long)
            .collect();
        for name in self.accepted_extra {
            let flag = crate::cli_command_registry::global_flag(name)
                .expect("accepted_extra names a declared global flag");
            let target = if flag.value.is_some() {
                &mut value
            } else {
                &mut boolean
            };
            target.push(name);
        }
        let repeating: Vec<&'static str> = self
            .documented
            .iter()
            .filter(|flag| flag.value.is_some() && flag.repeatable)
            .map(|flag| flag.long)
            .collect();
        FlagSpec::new(&value, &boolean).repeating(&repeating)
    }

    /// The documented flags marked required, in print order.
    #[must_use]
    pub(crate) fn required_flags(&self) -> Vec<&'static str> {
        self.documented
            .iter()
            .filter(|flag| flag.required)
            .map(|flag| flag.long)
            .collect()
    }
}

#[cfg(test)]
#[path = "cli_command_spec_tests.rs"]
mod tests;

#[path = "cli_command_spec_options.rs"]
mod options;
