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
//! - The listen request interception and its refusals
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

use crate::event_stream::{EventStream, FrameSource, frame};
use crate::http_post::{PostBody, PostResponse};
use crate::http_read::HttpRequest;
use crate::protocol::{ERR_INTERNAL, ERR_INVALID_REQUEST, RpcError, error_response};
use crate::runtime::McpState;
use crate::tool_signals::{NotRegistered, Subscriber, ToolSignals};
use crate::tools_page::current_view_digest;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// The request method this module answers.
const LISTEN_METHOD: &str = "subscriptions/listen";

/// The request metadata key naming the subscription a notification belongs to.
const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

/// Intercept one listen request, or `None` for every other body.
///
/// A body that is not JSON, or whose method is not `subscriptions/listen`,
/// returns `None` so the normal admission path runs unchanged.
pub(crate) fn intercept(state: &McpState, req: &HttpRequest) -> Option<PostResponse> {
    let body = serde_json::from_str::<Value>(req.body.trim()).ok()?;
    if body.get("method").and_then(Value::as_str) != Some(LISTEN_METHOD) {
        return None;
    }
    let id = match body.get("id") {
        Some(id @ (Value::String(_) | Value::Number(_))) => id,
        Some(_) => {
            return Some(buffered_error(
                None,
                ERR_INVALID_REQUEST,
                "Invalid request: a subscription id must be a string or a number",
            ));
        }
        None => {
            return Some(buffered_error(
                None,
                ERR_INVALID_REQUEST,
                "Invalid request: a subscription needs an `id`",
            ));
        }
    };
    let tools_list_changed = body
        .pointer("/params/notifications/toolsListChanged")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let baseline = match current_view_digest(state) {
        Ok(digest) => digest,
        Err(err) => {
            return Some(buffered_error(Some(id), ERR_INTERNAL, err.message));
        }
    };
    let key = match id {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => unreachable!("the id shape is checked above"),
    };
    let subscriber = match state.tool_signals.register(&key, baseline) {
        Ok(subscriber) => subscriber,
        Err(NotRegistered::Capacity) => {
            return Some((
                503,
                PostBody::Buffered(NotRegistered::Capacity.to_string()),
                Vec::new(),
            ));
        }
    };
    let seen = subscriber.observed();
    let acknowledgement = acknowledged(id, tools_list_changed);
    Some((
        200,
        PostBody::Stream(EventStream {
            head: frame(&acknowledgement.to_string()),
            source: Box::new(ListenSource {
                subscriber,
                id: id.clone(),
                key,
                signals: Arc::clone(&state.tool_signals),
                seen,
                tools_list_changed,
            }),
        }),
        Vec::new(),
    ))
}

/// A buffered JSON-RPC error answer.
fn buffered_error(id: Option<&Value>, code: i64, message: impl Into<String>) -> PostResponse {
    let err = RpcError::new(code, message);
    (
        200,
        PostBody::Buffered(error_response(id, &err).to_string()),
        Vec::new(),
    )
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
    /// The registry key the identity renders to.
    key: String,
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
        self.signals.unregister(&self.key);
    }
}

#[cfg(test)]
#[path = "tool_listen_tests.rs"]
mod tool_listen_tests;
