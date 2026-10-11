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
//! - Refusing an argument value the tool's schema does not type
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
//! Keep this module focused on the argument contract.

use crate::protocol::McpHandler;
use clio_types::{AmError, ErrorCode};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Arguments the runtime reads for every tool, whether or not a schema lists them:
/// `bank` and `actor` (`resolve_ctx`) and `timestamp` (`resolve_timestamp`). They
/// are used, not dropped, so they are never an unknown argument.
const RUNTIME_ARGUMENTS: &[&str] = &["bank", "actor", "timestamp"];

/// One JSON type a published property may declare.
///
/// `null` is absent here on purpose: an explicit null reads as absent in every
/// argument reader in this crate, so it is not a type fault. A `type` this
/// build does not know leaves its argument to the handler, so an unrecognised
/// schema shape cannot refuse every call by accident.
#[derive(Clone, Copy)]
enum JsonType {
    String,
    Number,
    Integer,
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
            "integer" => Some(Self::Integer),
            "boolean" => Some(Self::Boolean),
            "array" => Some(Self::Array),
            "object" => Some(Self::Object),
            _ => None,
        }
    }

    /// Whether `value` is of this type. `integer` accepts any number with no
    /// fractional part, which is the JSON Schema rule and not the JSON spelling.
    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Integer => value.as_f64().is_some_and(|n| n.fract() == 0.0),
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
            Self::Integer => "an integer",
            Self::Boolean => "a boolean",
            Self::Array => "an array",
            Self::Object => "an object",
        }
    }
}

/// The JSON types each declared property accepts, by argument name.
///
/// A property with no `type`, or with one this build does not know, is absent
/// from the map: its value is left to the handler.
fn declared_types(properties: Option<&Map<String, Value>>) -> HashMap<String, Vec<JsonType>> {
    let mut types = HashMap::new();
    for (key, property) in properties.into_iter().flatten() {
        let names: Vec<JsonType> = match property.get("type") {
            Some(Value::String(name)) => JsonType::parse(name).into_iter().collect(),
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .filter_map(JsonType::parse)
                .collect(),
            _ => Vec::new(),
        };
        if !names.is_empty() {
            types.insert(key.clone(), names);
        }
    }
    types
}

/// What one tool's published schema declares about its arguments.
struct Contract {
    /// Declared argument names, or `None` when the schema declares none.
    declared: Option<HashSet<String>>,
    /// The types each declared property accepts.
    types: HashMap<String, Vec<JsonType>>,
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
        let types = declared_types(properties);
        Self {
            declared: properties.map(|props| props.keys().cloned().collect()),
            types,
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
/// # Errors
/// An argument the schema does not declare, a missing required argument, a
/// required string that is empty or whitespace-only, or a declared argument
/// whose JSON type the schema does not accept.
pub(crate) fn check(name: &str, args: &Value) -> Result<(), AmError> {
    let Some(contract) = contracts().get(name) else {
        return Ok(());
    };
    let empty = Map::new();
    let fields = args.as_object().unwrap_or(&empty);
    if let Some(declared) = &contract.declared
        && let Some(unknown) = fields.keys().find(|key| {
            !declared.contains(key.as_str()) && !RUNTIME_ARGUMENTS.contains(&key.as_str())
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
        let Some(types) = contract.types.get(key) else {
            continue;
        };
        // An explicit null reads as absent, which is what every argument reader
        // in this crate already does, so it is not a type fault.
        if value.is_null() || types.iter().any(|declared| declared.accepts(value)) {
            continue;
        }
        let expected = types
            .iter()
            .map(|declared| declared.name())
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(AmError::new(
            ErrorCode::InvalidType,
            format!("{key} must be {expected}"),
        ));
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
