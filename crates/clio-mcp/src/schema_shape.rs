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
//! - The flattening pass: inline every local `$ref`, drop every `$defs`, and
//!   keep the definitions any reference that cannot be inlined still resolves
//!   against
//! - The local JSON Pointer walk that decides what a `$ref` names
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

/// How many references one chain may expand through before the chain is
/// treated as recursive and left as written. The bound keeps a schema that
/// refers back to itself, directly or through other definitions, from
/// recursing without end.
const MAX_REFERENCE_CHAIN: usize = 64;

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
/// at, and every `$defs` dropped, recursively.
///
/// A reference is inlined wherever it can be, and is otherwise left exactly as
/// written: an external URL, a local pointer with no target, a reference whose
/// pointer names an ancestor of itself, or a reference that carries a sibling
/// keyword. A `$ref` beside a sibling keyword stays as written rather than
/// being replaced, because the sibling applies alongside the reference.
///
/// Because a reference can survive, the definitions it resolves against can
/// too: when a remaining reference names a location inside a `$defs`
/// container, every `$defs` in the document is kept verbatim beside the
/// flattened content, so the output holds either no reference at all or the
/// definitions every remaining reference resolves against. No reference is
/// ever emitted against a definition this pass removed.
#[must_use]
pub fn flatten(schema: &Value) -> Value {
    let dropped = Walker::new(schema, false).walk();
    if dropped.surviving.is_empty()
        || !dropped
            .surviving
            .iter()
            .any(|reference| names_a_definition(schema, reference))
    {
        return dropped.value;
    }
    Walker::new(schema, true).walk().value
}

/// The result of one flattening pass: the rewritten document, and the
/// references that were left as written.
struct Walk {
    value: Value,
    surviving: Vec<String>,
}

/// One flattening pass over a single document.
struct Walker<'a> {
    /// The document every local reference in this pass resolves inside.
    root: &'a Value,
    /// Whether `$defs` containers are kept verbatim instead of dropped.
    keep_defs: bool,
    /// The references whose definitions are being inlined above this node, so
    /// a reference that names one of them again is a chain, not a target.
    expanding: Vec<String>,
    /// The references this pass left as written.
    surviving: Vec<String>,
}

impl<'a> Walker<'a> {
    fn new(root: &'a Value, keep_defs: bool) -> Self {
        Self {
            root,
            keep_defs,
            expanding: Vec::new(),
            surviving: Vec::new(),
        }
    }

    fn walk(mut self) -> Walk {
        let value = self.node(self.root);
        Walk {
            value,
            surviving: self.surviving,
        }
    }

    /// `node` rewritten: a reference is replaced by what it names, an array is
    /// rewritten element by element, and every other value is copied.
    fn node(&mut self, node: &Value) -> Value {
        match node {
            Value::Object(map) => self.object(map),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.node(item)).collect()),
            other => other.clone(),
        }
    }

    fn object(&mut self, map: &Map<String, Value>) -> Value {
        if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
            if let Some(target) = self.target(reference, map) {
                self.expanding.push(reference.to_owned());
                let expanded = self.node(target);
                self.expanding.pop();
                return expanded;
            }
            self.surviving.push(reference.to_owned());
            return Value::Object(map.clone());
        }
        let mut inline = Map::new();
        for (key, value) in map {
            if key == "$defs" && !self.keep_defs {
                continue;
            }
            inline.insert(key.clone(), self.node(value));
        }
        Value::Object(inline)
    }

    /// What `reference` stands for, when this node may be replaced by it at
    /// all. A `$ref` object with any sibling keyword is never replaced, and
    /// neither is a reference already being expanded above this node nor one
    /// past the chain bound, so a recursive document resolves to a finite
    /// rewrite instead of recursing without end.
    fn target(&self, reference: &str, map: &Map<String, Value>) -> Option<&'a Value> {
        let recursive = map.len() > 1
            || self.expanding.len() >= MAX_REFERENCE_CHAIN
            || self.expanding.iter().any(|seen| seen == reference);
        if recursive {
            return None;
        }
        pointed(self.root, reference)
    }
}

/// The tokens of one local `#/...` pointer, unescaped, or `None` when the
/// reference is not a local pointer.
fn local_tokens(reference: &str) -> Option<Vec<String>> {
    let path = reference.strip_prefix('#')?.strip_prefix('/')?;
    Some(
        path.split('/')
            .map(|raw| raw.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// The value one local pointer names inside `root`, or `None` when the pointer
/// is not local or does not resolve there.
///
/// An empty fragment and a `#` fragment both name the document root. Every
/// other fragment walks one token at a time: a token addresses a member when
/// the node it lands on is an object, and an index when the node is an array.
fn pointed<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if reference.is_empty() || reference == "#" {
        return Some(root);
    }
    let mut node = root;
    for token in local_tokens(reference)? {
        node = match node {
            Value::Object(map) => map.get(token.as_str())?,
            Value::Array(items) => items.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(node)
}

/// True when `reference` resolves against `root` inside a `$defs` container,
/// so the definitions it names have to stay beside it.
fn names_a_definition(root: &Value, reference: &str) -> bool {
    pointed(root, reference).is_some()
        && local_tokens(reference).is_some_and(|tokens| tokens.iter().any(|token| token == "$defs"))
}

#[cfg(test)]
#[path = "schema_shape_tests.rs"]
mod schema_shape_tests;
