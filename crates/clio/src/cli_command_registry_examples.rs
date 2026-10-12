// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static contract over one record's usage
//! examples: their count, their `clio` spelling, and whether each line names a
//! command the record carries and invokes it the way that command's parser
//! does.
//!
//! ## Owns
//! - `check_examples` / `check_example`, the per-record example contract
//! - `split_line`, the quote-aware tokenizer every example line is read with
//! - `examples_dump_json`, the test-only JSON seam the example evidence reads
//!
//! ## Does not own
//! - The record model and its fields (see `cli_command_spec`)
//! - The command records themselves (see the `cli_command_registry_*` record
//!   modules)
//! - The argument grammar each example is parsed with (see `cli_args`)
//!
//! ## Boundary
//!
//! Changes to what makes an example valid should normally require changes here.
//! Changes to one command's flags should not require changes here.
//!
//! Keep this module focused on the example contract.

use crate::cli_command_spec::{CommandSpec, Example};

/// The fewest examples a record may carry.
const MIN_EXAMPLES: usize = 2;
/// The most examples a record may carry.
const MAX_EXAMPLES: usize = 4;
/// The longest registered command name, in tokens; no record name is longer,
/// so the resolver never reads past this window.
const MAX_COMMAND_TOKENS: usize = 3;

/// Check one record's examples: the count, then every line.
///
/// Rejects, naming the record, a list shorter than two or longer than four,
/// and hands each line to `check_example`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn check_examples(spec: &CommandSpec) -> Result<(), String> {
    if !(MIN_EXAMPLES..=MAX_EXAMPLES).contains(&spec.examples.len()) {
        return Err(format!(
            "`{}` carries {} examples; every record needs {MIN_EXAMPLES} to {MAX_EXAMPLES}",
            spec.name,
            spec.examples.len()
        ));
    }
    for example in spec.examples {
        check_example(spec, example)?;
    }
    Ok(())
}

/// Check one example line: it must not be blank, must start with `clio `, must
/// resolve to a command the carrying record owns — the record itself, one of
/// its aliases, or a record that lists it as an alias — and must pass that
/// command's own parser and positional arity. Leading global flags are lifted
/// the way the top level lifts them, so `clio --db x get m1` resolves like
/// `clio get m1 --db x`.
///
/// The accepted flags are the named command's documented flags, its
/// `accepted_extra` globals, and every declared global flag — exactly the set
/// its parser spec accepts, the short spelling of `--output` included, because
/// the line is parsed with the same `cli_args::parse` the command runs.
///
/// The documented alternate spellings parse too: a documented value-optional
/// flag may appear bare, and a record's stand-in flag takes the place of its
/// required positional (`admit --file`, and `backup restore|verify --latest`).
///
/// A line naming a command the record does not carry, a flag the command does
/// not accept, a value or repeat misuse, a token that is not an argument, a
/// missing required positional, and an extra positional are all rejections,
/// and the failure quotes the record and the example text.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn check_example(spec: &CommandSpec, example: &Example) -> Result<(), String> {
    let context = format!("`{}` example `{}`", spec.name, example.command);
    if example.command.trim().is_empty() {
        return Err(format!("{context} is blank"));
    }
    if !example.command.starts_with("clio ") {
        return Err(format!("{context} does not start with `clio `"));
    }
    let tokens = split_line(example.command).map_err(|reason| format!("{context} {reason}"))?;
    let (rest, globals) = crate::cli_args::split_leading_globals(&tokens[1..]);
    let Some((target, args)) = resolve(&rest) else {
        return Err(format!("{context} names no known command"));
    };
    if !carried_by(spec, target) {
        return Err(format!(
            "{context} names `{}`, which `{}` does not carry",
            target.name, spec.name
        ));
    }
    let merged = crate::cli_args::merge_leading_globals(args.iter().cloned(), globals);
    let args = without_bare_optional_values(target, merged);
    let parsed = crate::cli_args::parse(&target.parser_spec(), args)
        .map_err(|error| format!("{context}: {}", error.message()))?;
    let given = parsed.positionals().len();
    if given > target.positionals.len() {
        let extra = parsed
            .positional(target.positionals.len())
            .unwrap_or_default();
        return Err(format!("{context} has an extra argument `{extra}`"));
    }
    if positional_stand_in_supplied(target, &parsed) {
        return Ok(());
    }
    if let Some(missing) = target
        .positionals
        .iter()
        .skip(given)
        .find(|positional| positional.required)
    {
        return Err(format!(
            "{context} is missing the required positional `{}`",
            missing.label
        ));
    }
    Ok(())
}

/// Whether the carrying record owns the command an example resolves to: the
/// target is the record itself, one of its aliases, or a record that lists it
/// as an alias, so a line only ever documents the record that carries it.
fn carried_by(spec: &CommandSpec, target: &CommandSpec) -> bool {
    target.name == spec.name
        || spec.aliases.contains(&target.name)
        || target.aliases.contains(&spec.name)
}

/// The arguments with a bare documented value-optional flag removed: the value
/// may be omitted, and a following flag token is never its value, so the line
/// parses like the command's own bare spelling. The strip follows the real
/// grammar's hard edges: a token from the `--` delimiter on is a positional,
/// never a flag, and a flag spelled twice is the duplicate the parser refuses,
/// so only a single pre-delimiter occurrence is re-spelled.
fn without_bare_optional_values(target: &CommandSpec, args: Vec<String>) -> Vec<String> {
    let optional: Vec<&str> = target
        .documented
        .iter()
        .filter(|flag| flag.value_optional)
        .map(|flag| flag.long)
        .collect();
    if optional.is_empty() {
        return args;
    }
    // Everything from the delimiter on is positional, so it is never a flag
    // occurrence and never re-spelled.
    let flags_end = args
        .iter()
        .position(|given| given.as_str() == "--")
        .unwrap_or(args.len());
    let mut kept = Vec::with_capacity(args.len());
    for (index, token) in args.iter().enumerate() {
        let bare = index < flags_end
            && token
                .strip_prefix("--")
                .is_some_and(|name| optional.contains(&name))
            && args
                .get(index + 1)
                .is_none_or(|next| next.starts_with("--"))
            && args[..flags_end]
                .iter()
                .filter(|given| given.as_str() == token.as_str())
                .count()
                == 1;
        if !bare {
            kept.push(token.clone());
        }
    }
    kept
}

/// The documented flag that takes a record's required positional's place when
/// its alternate usage line is used: `admit` reads the text from `--file`, and
/// the backup selectors read the snapshot id from `--latest`.
const POSITIONAL_STANDINS: &[(&str, &str)] = &[
    ("admit", "file"),
    ("backup restore", "latest"),
    ("backup verify", "latest"),
];

/// Whether the parsed line supplies the target's documented positional
/// stand-in, so the line is complete without the positional.
fn positional_stand_in_supplied(
    target: &CommandSpec,
    parsed: &crate::cli_args::ParsedArgs,
) -> bool {
    POSITIONAL_STANDINS.iter().any(|(record, flag)| {
        *record == target.name
            && target
                .documented
                .iter()
                .any(|documented| documented.long == *flag)
            && (parsed.has(flag) || parsed.value(flag).is_some())
    })
}

/// The command an example's tokens name, with the tokens that are its own
/// arguments: the longest registered name among the leading tokens, matching a
/// visible name or an alias. `clio task get t1` resolves through `task get`.
fn resolve(tokens: &[String]) -> Option<(&'static CommandSpec, &[String])> {
    for length in (1..=tokens.len().min(MAX_COMMAND_TOKENS)).rev() {
        let name = tokens[..length].join(" ");
        if let Some(spec) = super::resolve(&name) {
            return Some((spec, &tokens[length..]));
        }
    }
    None
}

/// Split an example line into tokens: whitespace separates, single and double
/// quotes group, and a backslash escapes the next character inside double
/// quotes and outside both kinds of quotes (inside single quotes it stays
/// literal). A quote left open is a refusal, not a silent regrouping.
fn split_line(line: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if quote == Some('\'') {
            if character == '\'' {
                quote = None;
            } else {
                current.push(character);
            }
        } else if quote == Some('"') {
            if character == '"' {
                quote = None;
            } else if character == '\\' {
                escaped = true;
            } else {
                current.push(character);
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
            started = true;
        } else if character == '\\' {
            escaped = true;
            started = true;
        } else if character.is_whitespace() {
            if started {
                tokens.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(character);
            started = true;
        }
    }
    if quote.is_some() {
        return Err(String::from("leaves a quote open"));
    }
    if escaped {
        current.push('\\');
    }
    if started {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Every record's examples as one compact JSON array: one object per example,
/// in registry and record order, with `record`, `command`, and `comment` keys
/// and `null` for an absent comment. Test-only: the env-gated evidence seam
/// reads it.
#[cfg(test)]
pub(crate) fn examples_dump_json(specs: &[&CommandSpec]) -> String {
    let rows: Vec<String> = specs
        .iter()
        .flat_map(|spec| {
            spec.examples.iter().map(move |example| {
                format!(
                    "{{\"record\": {}, \"command\": {}, \"comment\": {}}}",
                    json_string(spec.name),
                    json_string(example.command),
                    example
                        .comment
                        .map_or_else(|| String::from("null"), json_string),
                )
            })
        })
        .collect();
    format!("[{}]", rows.join(", "))
}

/// A string as its JSON literal, quotes and escaping included.
#[cfg(test)]
fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

#[cfg(test)]
#[path = "cli_command_registry_examples_tests.rs"]
mod tests;
