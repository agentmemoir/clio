// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `subscriptions/listen` request: the
//! acknowledgement the stream opens with, the list-change frames that follow,
//! and the registry entry that stops when the client closes the stream.
//!
//! ## Owns
//! - The listen request handler and its refusals
//! - The acknowledgement envelope and the later list-change envelope
//! - The streamed source that turns published changes into frames
//!
//! ## Does not own
//! - The change counter and the subscriber registry (see `tool_signals`)
//! - The view digest (see `tools_page`)
//! - SSE framing, pacing, and socket writes (see `event_stream` and `http`)
//!
//! ## Boundary
//!
//! Changes to what a listen request answers should normally require changes
//! here. Changes to the change signal or the socket should not.
//!
//! Keep this module focused on the listen request and its stream.

use crate::conditional_http::refusal;
use crate::event_stream::{EventStream, FrameSource, frame};
use crate::http_post::{PostBody, PostResponse};
use crate::protocol::{ERR_INTERNAL, ERR_INVALID_REQUEST, RpcError, invalid_params};
use crate::runtime::McpState;
use crate::tool_signals::{NotRegistered, Subscriber, ToolSignals};
use crate::tools_page::current_view_digest;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// The request method this module answers.
pub(crate) const LISTEN_METHOD: &str = "subscriptions/listen";

/// The request metadata key naming the subscription a notification belongs to.
const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

/// Serve one admitted listen request.
///
/// The transport validates the request first, so every admission gate has
/// already run. A request this module refuses answers the shared refusal status
/// and JSON-RPC body, and registers no subscriber.
pub(crate) fn serve(state: &McpState, body: &Value) -> PostResponse {
    let Some(id) = body.get("id") else {
        return refused(
            body,
            &RpcError::new(
                ERR_INVALID_REQUEST,
                "Invalid request: a subscription needs an `id`",
            ),
        );
    };
    if !matches!(id, Value::String(_) | Value::Number(_)) {
        return refused(
            &Value::Null,
            &RpcError::new(
                ERR_INVALID_REQUEST,
                "Invalid request: a subscription id must be a string or a number",
            ),
        );
    }
    let tools_list_changed = match tools_list_changed(body) {
        Ok(value) => value,
        Err(err) => return refused(body, &err),
    };
    let baseline = match current_view_digest(state) {
        Ok(digest) => digest,
        Err(err) => return refused(body, &err),
    };
    let (serial, subscriber) = match state.tool_signals.register(baseline) {
        Ok(registered) => registered,
        Err(NotRegistered::Capacity) => {
            return refused(
                body,
                &RpcError::new(ERR_INTERNAL, NotRegistered::Capacity.to_string()),
            );
        }
    };
    let seen = subscriber.observed();
    let acknowledgement = acknowledged(id, tools_list_changed);
    (
        200,
        PostBody::Stream(EventStream {
            head: frame(&acknowledgement.to_string()),
            source: Box::new(ListenSource {
                subscriber,
                id: id.clone(),
                serial,
                signals: Arc::clone(&state.tool_signals),
                seen,
                tools_list_changed,
            }),
        }),
        Vec::new(),
    )
}

/// Whether the client asked for tool-list changes, or the refusal for a filter
/// the revision does not type.
fn tools_list_changed(body: &Value) -> Result<bool, RpcError> {
    match body.pointer("/params/notifications") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Object(notifications)) => match notifications.get("toolsListChanged") {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(value)) => Ok(*value),
            Some(_) => Err(invalid_params(
                "Invalid params: `notifications.toolsListChanged` must be a boolean",
            )),
        },
        Some(_) => Err(invalid_params(
            "Invalid params: `params.notifications` must be an object",
        )),
    }
}

/// A buffered refusal: the shared status and JSON-RPC error body.
fn refused(body: &Value, err: &RpcError) -> PostResponse {
    let (status, text) = refusal(body, err);
    (status, PostBody::Buffered(text), Vec::new())
}

/// The notification a stream opens with.
pub(crate) fn acknowledged(id: &Value, tools_list_changed: bool) -> Value {
    let notifications = if tools_list_changed {
        json!({ "toolsListChanged": true })
    } else {
        json!({})
    };
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/subscriptions/acknowledged",
        "params": {
            "_meta": { (SUBSCRIPTION_ID): id },
            "notifications": notifications
        }
    })
}

/// One notification that the server's tool list changed.
pub(crate) fn list_changed(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/tools/list_changed",
        "params": { "_meta": { (SUBSCRIPTION_ID): id } }
    })
}

/// The streamed source of list-change frames for one subscription.
struct ListenSource {
    /// The condition this source waits on.
    subscriber: Arc<Subscriber>,
    /// The subscription identity: the listen request's own `id`.
    id: Value,
    /// The registry serial this stream holds.
    serial: u64,
    /// The registry that holds this subscription.
    signals: Arc<ToolSignals>,
    /// The change count this source already emitted.
    seen: u64,
    /// Whether the client asked for tool-list changes.
    tools_list_changed: bool,
}

impl FrameSource for ListenSource {
    fn next(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        let now = self.subscriber.wait_from(self.seen, timeout);
        if now == self.seen {
            return None;
        }
        let burst = now - self.seen;
        self.seen = now;
        if !self.tools_list_changed {
            return None;
        }
        let one = frame(&list_changed(&self.id).to_string());
        let mut frames = Vec::new();
        for _ in 0..burst {
            frames.extend_from_slice(&one);
        }
        Some(frames)
    }
}

impl Drop for ListenSource {
    fn drop(&mut self) {
        self.signals.unregister(self.serial);
    }
}

#[cfg(test)]
#[path = "tool_listen_tests.rs"]
mod tool_listen_tests;
