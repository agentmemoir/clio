// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `tool_invoke` wrapper: it checks the
//! wrapper arguments against the published declaration, refuses a name that
//! cannot be invoked, and answers with the target tool's own `tools/call`
//! result unchanged.
//!
//! ## Owns
//! - The `mcp.discovery` flag that decides whether the wrapper is reachable
//! - Checking the wrapper arguments, and the refusal a failure produces
//! - Refusing a discovery tool or a name no catalog binds, before dispatch
//! - The recursion refusal a discovery name gets at the shared execute path
//!
//! ## Does not own
//! - Executing the target, counting it, or shaping its response (see `protocol`)
//! - What a business tool accepts (see `arg_contract`)
//! - The published declarations of the three discovery tools (see
//!   `discovery_tool_defs`)
//!
//! ## Boundary
//!
//! Changes to how a wrapped call is validated, refused, or routed should
//! normally require changes here. Changes to what a target tool accepts or
//! returns should not: the target's own response is handed back untouched.
//!
//! Keep this module focused on the wrapper, and leave the target's response
//! alone.

use crate::protocol::{McpHandler, RpcError, invalid_params, tool_call_envelope};
use crate::runtime::McpState;
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};

/// The wrapper tool this module serves.
pub(crate) const TOOL_INVOKE: &str = "tool_invoke";

/// Whether this deployment exposes the discovery tools.
///
/// `mcp.discovery` absent, off, or unreadable keeps them off, which is the
/// recorded default: a deployment that never sets the key neither lists the
/// helpers nor calls one by name.
pub(crate) fn enabled(state: &McpState) -> bool {
    let Ok(runtime) = state.config.lock() else {
        return false;
    };
    runtime
        .effective_raw()
        .pointer("/mcp/discovery")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Whether the embedded discovery document declares `name`.
///
/// The document is the one place the three helper names are written, so this
/// predicate cannot drift from the declarations the listing publishes.
fn is_discovery(name: &str) -> bool {
    crate::discovery_tool_defs::definitions()
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
}

/// The refusal for a name the wrapper will not hand to dispatch, or `None` when
/// the name may run.
///
/// A discovery target would re-enter the wrapper family, and a name no catalog
/// binds has no handler to reach, so both are refused with the name in the
/// message and nothing recorded.
fn refused_target(target: &str) -> Option<AmError> {
    if is_discovery(target) {
        return Some(AmError::new(
            ErrorCode::InvalidArgument,
            format!("`{target}` is a discovery tool, so `{TOOL_INVOKE}` will not call it"),
        ));
    }
    if !crate::bound_tools().contains(&target) {
        return Some(AmError::new(
            ErrorCode::InvalidArgument,
            format!("`{target}` is not a tool this build binds"),
        ));
    }
    None
}

/// The recursion refusal for `name`, or `None` when it is a business tool.
///
/// Every name-routed dispatch passes through the shared execute path, so the
/// guard there refuses a discovery name whatever route named it — a direct
/// call, or a chain that reached the path indirectly.
pub(crate) fn recursion_refusal(name: &str) -> Option<RpcError> {
    if is_discovery(name) {
        return Some(invalid_params(format!("unknown tool `{name}`")));
    }
    None
}

/// Answer a failure of ours as a `tools/call` result, shaped for `name`.
///
/// The envelope is the same one a direct call builds: `name` decides whether
/// the payload travels in `structuredContent` or in the text block.
fn refusal(name: &str, err: AmError) -> Value {
    let result = crate::write_tools::tool_result(Err(err));
    let has_schema = crate::schema_output_defs::output_schema(name).is_some();
    tool_call_envelope(&result, has_schema)
}

/// Serve one `tool_invoke` `tools/call`, whose `params` carry the wrapper
/// arguments.
///
/// The wrapper is validated by the same argument contract a direct call uses,
/// and its failure names the wrapper and the failing field. A target that
/// cannot be invoked is refused before anything is recorded. A target that can
/// is answered by the shared execute path, so the response is the one a direct
/// call to that target produces, byte for byte, and only the target is counted.
///
/// # Errors
/// The target's result reports a failed co-activation durability handoff.
pub(crate) fn invoke(state: &McpState, params: &Value) -> Result<Value, RpcError> {
    let empty = json!({});
    let wrapper_args = params.get("arguments").unwrap_or(&empty);
    if let Err(err) = crate::arg_contract::check(TOOL_INVOKE, wrapper_args) {
        let message = format!("{TOOL_INVOKE}: {}", err.message);
        return Ok(refusal(TOOL_INVOKE, AmError::new(err.code, message)));
    }
    let target = wrapper_args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // `{}` is the absent-arguments default the declaration publishes.
    let target_args = wrapper_args
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(err) = refused_target(target) {
        return Ok(refusal(TOOL_INVOKE, err));
    }
    if let Err(err) = crate::arg_contract::check(target, &target_args) {
        // A fault the contract already attributes to the tool keeps its message,
        // so the target is named once and the failing field is still named.
        let message = if err.message.contains(target) {
            err.message
        } else {
            format!("{target}: {}", err.message)
        };
        return Ok(refusal(target, AmError::new(err.code, message)));
    }
    McpHandler::execute_tool_call(state, target, &target_args)
}

#[cfg(test)]
#[path = "catalog_invoke_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "catalog_invoke_examples_tests.rs"]
mod examples_tests;

#[cfg(test)]
#[path = "catalog_invoke_confirmation_tests.rs"]
mod confirmation_tests;

#[cfg(test)]
#[path = "catalog_invoke_gate_fixture_tests.rs"]
mod gate_fixture_tests;
