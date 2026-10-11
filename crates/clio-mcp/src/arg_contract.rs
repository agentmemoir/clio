// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP argument contract: the checks each
//! `tools/call` passes before its tool runs, taken from the tool's published
//! input schema.
//!
//! ## Owns
//! - Refusing an argument name the tool's schema does not declare
//! - Refusing an argument value the tool's schema does not type, and an array
//!   entry whose type the schema's `items` does not accept
//! - Refusing a missing required argument and a blank required string
//!
//! ## Does not own
//! - Per-tool value rules such as timestamps, categories, or admission gates
//! - The schema pack itself (see `schema_defs`)
//!
//! ## Boundary
//!
//! Changes to which arguments a tool accepts should normally require changes to
//! its schema. Changes to how the contract is enforced should not require
//! changes in the tool handlers.
//!
//! The runtime arguments `bank`, `actor`, and `timestamp` are the one exception:
//! the runtime reads them for every tool and most schemas declare none of them,
//! so their types live here rather than in the pack.
//!
//! Keep this module focused on the argument contract.

use crate::protocol::McpHandler;
use clio_types::{AmError, ErrorCode};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Arguments the runtime reads for every tool, whether or not a schema lists
/// them, with the type the runtime parses: `bank` and `actor` (`resolve_ctx`)
/// and `timestamp` (`resolve_timestamp`). They are used, not dropped, so they
/// are never an unknown argument; and they are checked whether or not the tool
/// declares them, because a value the runtime cannot parse is discarded and the
/// call still reports success.
const RUNTIME_ARGUMENTS: &[(&str, JsonType)] = &[
    ("bank", JsonType::String),
    ("actor", JsonType::String),
    ("timestamp", JsonType::String),
];

/// One JSON type a published node may declare.
///
/// `null` is absent here on purpose: an explicit null reads as absent in every
/// argument reader in this crate, so it is not a type fault. A `type` this
/// build does not know leaves its value to the handler, so an unrecognised
/// schema shape cannot refuse every call by accident.
#[derive(Clone, Copy)]
enum JsonType {
    String,
    Number,
    Boolean,
    Array,
    Object,
}

impl JsonType {
    /// The type named by a schema `type`, or `None` when the name is not one
    /// this build checks.
    fn parse(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            "array" => Some(Self::Array),
            "object" => Some(Self::Object),
            _ => None,
        }
    }

    /// Whether `value` is of this type.
    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Array => value.is_array(),
            Self::Object => value.is_object(),
        }
    }

    /// The clause a message completes: `limit must be a number`.
    fn name(self) -> &'static str {
        match self {
            Self::String => "a string",
            Self::Number => "a number",
            Self::Boolean => "a boolean",
            Self::Array => "an array",
            Self::Object => "an object",
        }
    }
}

/// The type names one schema node declares, in the string form or the array
/// form, where a union accepts a value of any type it lists. `None` when the
/// node declares no type this build knows.
fn types_of(node: &Value) -> Option<Vec<JsonType>> {
    let names: Vec<JsonType> = match node.get("type") {
        Some(Value::String(name)) => JsonType::parse(name).into_iter().collect(),
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_str)
            .filter_map(JsonType::parse)
            .collect(),
        _ => Vec::new(),
    };
    (!names.is_empty()).then_some(names)
}

/// What a tool's schema says its arguments accept.
struct Accepts {
    /// The types each argument accepts, by argument name.
    values: HashMap<String, Vec<JsonType>>,
    /// The types each array argument's entries accept, by argument name.
    entries: HashMap<String, Vec<JsonType>>,
}

/// What each declared property accepts. A property with no `type`, or with one
/// this build does not know, is absent from `values`: its value is left to the
/// handler. The runtime arguments are always present, so a value the runtime
/// cannot parse is refused even where no schema declares it.
fn declared_types(properties: Option<&Map<String, Value>>) -> Accepts {
    let mut accepts = Accepts {
        values: HashMap::new(),
        entries: HashMap::new(),
    };
    for (key, property) in properties.into_iter().flatten() {
        if let Some(types) = types_of(property) {
            accepts.values.insert(key.clone(), types);
        }
        // `items` types an array's entries. An array whose entries are untyped
        // stays with its handler.
        if let Some(entry_types) = property.get("items").and_then(types_of) {
            accepts.entries.insert(key.clone(), entry_types);
        }
    }
    for (name, json_type) in RUNTIME_ARGUMENTS {
        accepts
            .values
            .entry((*name).to_owned())
            .or_insert_with(|| vec![*json_type]);
    }
    accepts
}

/// Whether the declared types accept one value. An explicit null reads as
/// absent, which is what every argument reader in this crate already does, so it
/// is never a type fault.
fn accepts(types: &[JsonType], value: &Value) -> bool {
    value.is_null() || types.iter().any(|declared| declared.accepts(value))
}

/// The refusal for one value the declared types do not accept. `key` names the
/// argument, or one entry of it as `key[index]`.
fn type_error(key: &str, types: &[JsonType]) -> AmError {
    let expected = types
        .iter()
        .map(|declared| declared.name())
        .collect::<Vec<_>>()
        .join(" or ");
    AmError::new(ErrorCode::InvalidType, format!("{key} must be {expected}"))
}

/// What one tool's published schema declares about its arguments.
struct Contract {
    /// Declared argument names, or `None` when the schema declares none.
    declared: Option<HashSet<String>>,
    /// The types each argument and each array entry accepts.
    accepts: Accepts,
    required: Vec<String>,
    required_strings: Vec<String>,
}

impl Contract {
    fn from_schema(schema: &Value) -> Self {
        let properties = schema.get("properties").and_then(Value::as_object);
        let required: Vec<String> = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let required_strings = required
            .iter()
            .filter(|key| {
                properties
                    .and_then(|props| props.get(key.as_str()))
                    .and_then(|prop| prop.get("type"))
                    .and_then(Value::as_str)
                    == Some("string")
            })
            .cloned()
            .collect();
        let accepts = declared_types(properties);
        Self {
            declared: properties.map(|props| props.keys().cloned().collect()),
            accepts,
            required,
            required_strings,
        }
    }
}

/// Every tool's contract, built once from the schema the server publishes.
fn contracts() -> &'static HashMap<String, Contract> {
    static CONTRACTS: OnceLock<HashMap<String, Contract>> = OnceLock::new();
    CONTRACTS.get_or_init(|| {
        McpHandler::tools_list()
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?;
                let schema = tool.get("inputSchema")?;
                Some((name.to_owned(), Contract::from_schema(schema)))
            })
            .collect()
    })
}

/// Check one `tools/call`'s arguments against the tool's contract. A tool with
/// no published contract is left to its handler.
///
/// The contract is the guarantee and the published schema is its only input, so
/// a property the schema leaves untyped, or types with a name this build does
/// not know, is not checked. A wrong type fails with `invalid_type` and names
/// the argument, because the readers that already answer this fault use that
/// code; `invalid_argument` stays the code for an argument that is missing,
/// blank, or not declared at all.
///
/// # Errors
/// An argument the schema does not declare, a missing required argument, a
/// required string that is empty or whitespace-only, or an argument, or an array
/// entry of one, whose JSON type the schema does not accept.
pub(crate) fn check(name: &str, args: &Value) -> Result<(), AmError> {
    let Some(contract) = contracts().get(name) else {
        return Ok(());
    };
    let empty = Map::new();
    let fields = args.as_object().unwrap_or(&empty);
    if let Some(declared) = &contract.declared
        && let Some(unknown) = fields.keys().find(|key| {
            !declared.contains(key.as_str())
                && !RUNTIME_ARGUMENTS
                    .iter()
                    .any(|(name, _)| *name == key.as_str())
        })
    {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "unknown argument `{unknown}` for `{name}`; tools/list names the accepted arguments"
            ),
        ));
    }
    for key in &contract.required {
        if !fields.contains_key(key) {
            return Err(required_error(key));
        }
    }
    for key in &contract.required_strings {
        match fields.get(key).and_then(Value::as_str) {
            Some("") => return Err(required_error(key)),
            Some(text) if text.trim().is_empty() => {
                return Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    format!("{key} must not be empty or whitespace-only"),
                ));
            }
            _ => {}
        }
    }
    for (key, value) in fields {
        let Some(types) = contract.accepts.values.get(key) else {
            continue;
        };
        if !accepts(types, value) {
            return Err(type_error(key, types));
        }
        let (Some(entry_types), Value::Array(entries)) = (contract.accepts.entries.get(key), value)
        else {
            continue;
        };
        for (index, entry) in entries.iter().enumerate() {
            if !accepts(entry_types, entry) {
                return Err(type_error(&format!("{key}[{index}]"), entry_types));
            }
        }
    }
    Ok(())
}

/// The message for a missing or empty required argument. It keeps the wording
/// the handlers already use, so a caller sees one message for one fault.
fn required_error(key: &str) -> AmError {
    AmError::new(ErrorCode::InvalidArgument, format!("{key} is required"))
}

#[cfg(test)]
#[path = "arg_contract_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "arg_contract_type_tests.rs"]
mod type_tests;
