// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving a tool-schema document to exactly
//! one pack shape: the reference form that ships local `$defs` beside the
//! `#/$defs/...` references that resolve against them, or the flattened form
//! that inlines every local reference and drops the definitions.
//!
//! ## Owns
//! - `PackShape`, the one shape this build ships, and the resolved form of a
//!   schema under each shape
//! - The flattening pass: inline every local `$ref`, drop every `$defs`
//! - The per-tool rewrite that puts a tool's `inputSchema` and `outputSchema`
//!   in the requested shape
//!
//! ## Does not own
//! - Assembling the pack document around the tools, or its hash (see `schema`)
//! - Emitting the local references and their definitions (`schema_defs`,
//!   `schema_retention_defs`)
//!
//! ## Boundary
//!
//! Changes to which pack shape ships, or to what a shape resolves to, should
//! normally require changes here. Changes to what a tool declares should not
//! require changes here.
//!
//! Keep this module focused on the pack shape and its single flattening pass.

use serde_json::{Map, Value};

/// The shape a tool-schema document is published in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackShape {
    /// A local `#/$defs/...` reference stays a reference, and the root `$defs`
    /// it resolves against ships beside it.
    Reference,
    /// Every local reference is replaced by the definition it points at, so
    /// the document needs no `$defs` and resolves alone.
    Flattened,
}

/// The shape this build ships.
///
/// The shape probe cannot run yet, so the flattened shape ships.
pub const SHIPPED_PACK_SHAPE: PackShape = PackShape::Flattened;

/// The tool keys that hold a schema document.
const SCHEMA_KEYS: [&str; 2] = ["inputSchema", "outputSchema"];

impl PackShape {
    /// `schema` in this shape.
    #[must_use]
    pub fn schema(self, schema: &Value) -> Value {
        match self {
            Self::Reference => schema.clone(),
            Self::Flattened => flatten(schema),
        }
    }

    /// `tool` with its `inputSchema` and `outputSchema` in this shape; every
    /// other field is copied unchanged.
    #[must_use]
    pub fn tool(self, tool: &Value) -> Value {
        let mut shaped = tool.clone();
        for key in SCHEMA_KEYS {
            if let Some(schema) = tool.get(key) {
                shaped[key] = self.schema(schema);
            }
        }
        shaped
    }
}

/// `schema` with every local reference replaced by the definition it points
/// at, and every `$defs` dropped, recursively, so the result holds neither and
/// each schema still resolves alone.
///
/// A reference the document does not resolve stays exactly as written: an
/// external URL, or a local pointer with no target, is never rewritten and no
/// definition is invented for it. A sibling keyword beside a resolved `$ref`
/// is replaced along with the reference, which is what the reference resolves
/// to on its own.
#[must_use]
pub fn flatten(schema: &Value) -> Value {
    flatten_node(schema, schema)
}

/// `node` flattened against `root`, the document its local references resolve
/// inside.
fn flatten_node(node: &Value, root: &Value) -> Value {
    match node {
        Value::Object(map) => {
            let target = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| pointed(root, reference));
            if let Some(target) = target {
                return flatten_node(target, root);
            }
            let mut inline = Map::new();
            for (key, value) in map {
                if key == "$defs" {
                    continue;
                }
                inline.insert(key.clone(), flatten_node(value, root));
            }
            Value::Object(inline)
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| flatten_node(item, root)).collect())
        }
        other => other.clone(),
    }
}

/// The value one local `#/...` pointer names inside `root`, or `None` when the
/// pointer is not local or does not resolve there.
fn pointed<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let mut node = root;
    for token in reference.strip_prefix("#/")?.split('/') {
        let token = token.replace("~1", "/").replace("~0", "~");
        node = match token.parse::<usize>() {
            Ok(index) => node.get(index)?,
            Err(_) => node.get(token.as_str())?,
        };
    }
    Some(node)
}

#[cfg(test)]
#[path = "schema_shape_tests.rs"]
mod schema_shape_tests;
