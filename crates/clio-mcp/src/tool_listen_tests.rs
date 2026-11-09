// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit tests of the `subscriptions/listen`
//! request: the acknowledgement, the honoured subset, the frames one publish
//! produces, the registry entry two identical ids share, the unregister on
//! drop, and the cursor a view change invalidates.
//!
//! ## Owns
//! - The acknowledgement and list-change envelope assertions
//! - The streamed source assertions, driven by the real registry
//! - The listing refusal after a view change
//!
//! ## Does not own
//! - The envelopes or the source (see `tool_listen`)
//! - The change signal (see `tool_signals`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::http_read::HttpRequest;
use crate::protocol::ERR_INVALID_PARAMS;
use crate::stateless_fixtures::state;
use std::collections::HashMap;

/// A request body the transport would carry for `body`.
fn request(body: &Value) -> HttpRequest {
    HttpRequest {
        method: "POST".to_owned(),
        path: "/mcp".to_owned(),
        headers: HashMap::new(),
        body: body.to_string(),
    }
}

/// One listen frame with `id` and the given opt-in.
fn listen_frame(id: &Value, tools_list_changed: bool) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "subscriptions/listen",
        "params": { "notifications": { "toolsListChanged": tools_list_changed } }
    })
}

/// The streamed body of a successful listen answer.
fn streamed(response: PostResponse) -> EventStream {
    let (status, body, extra) = response;
    assert_eq!(status, 200, "a listen request is served");
    assert!(extra.is_empty(), "a stream carries no extra headers");
    match body {
        PostBody::Stream(stream) => stream,
        PostBody::Buffered(other) => panic!("a listen answer is a stream: {other:?}"),
    }
}

/// The next frame, or `None` when the source has nothing to send.
fn next_frame(stream: &mut EventStream) -> Option<String> {
    stream
        .source
        .next(Duration::ZERO)
        .map(|bytes| String::from_utf8(bytes).expect("a frame is UTF-8"))
}

#[test]
fn the_acknowledgement_names_the_subscription_and_the_honoured_types() {
    let id = json!(7);
    let ack = acknowledged(&id, true);
    assert_eq!(ack["jsonrpc"], json!("2.0"));
    assert_eq!(
        ack["method"],
        json!("notifications/subscriptions/acknowledged")
    );
    assert_eq!(ack["params"]["_meta"][SUBSCRIPTION_ID], json!(7));
    assert_eq!(
        ack["params"]["notifications"],
        json!({ "toolsListChanged": true })
    );

    let state = state();
    let response = intercept(&state, &request(&listen_frame(&id, true)))
        .expect("a listen request is intercepted");
    let stream = streamed(response);
    assert_eq!(
        stream.head,
        frame(&ack.to_string()),
        "the stream opens with the acknowledgement frame"
    );
}

#[test]
fn a_client_that_opted_in_to_nothing_gets_no_frames() {
    let state = state();
    let response = intercept(&state, &request(&listen_frame(&json!(3), false)))
        .expect("a listen request is intercepted");
    let mut stream = streamed(response);
    assert_eq!(
        stream.head,
        frame(&acknowledged(&json!(3), false).to_string())
    );
    assert_eq!(
        acknowledged(&json!(3), false)["params"]["notifications"],
        json!({}),
        "the acknowledgement names no type the server would not send"
    );

    assert!(state.tool_signals.publish("view-two"));
    assert!(
        next_frame(&mut stream).is_none(),
        "an unrequested notification is not sent"
    );
    assert!(
        state.tool_signals.publish("view-three"),
        "the change still advances the source"
    );
    assert!(next_frame(&mut stream).is_none(), "still no frame");
}

#[test]
fn one_publish_yields_one_list_changed_frame() {
    let state = state();
    let response = intercept(&state, &request(&listen_frame(&json!(5), true)))
        .expect("a listen request is intercepted");
    let mut stream = streamed(response);

    assert!(state.tool_signals.publish("view-two"));
    let frame_text = next_frame(&mut stream).expect("one publish yields one frame");
    assert_eq!(
        frame_text.as_bytes(),
        frame(&list_changed(&json!(5)).to_string()).as_slice()
    );
    assert_eq!(
        frame_text
            .matches("\"method\":\"notifications/tools/list_changed\"")
            .count(),
        1,
        "{frame_text:?}"
    );
    assert!(frame_text.contains(SUBSCRIPTION_ID), "{frame_text:?}");
    assert!(next_frame(&mut stream).is_none(), "one publish, one frame");
}

#[test]
fn repeated_registrations_share_one_subscriber_entry() {
    let state = state();
    let first = intercept(&state, &request(&listen_frame(&json!("same"), true)))
        .expect("the first listen request is intercepted");
    let second = intercept(&state, &request(&listen_frame(&json!("same"), true)))
        .expect("the second listen request is intercepted");
    let _first = streamed(first);
    let _second = streamed(second);
    assert_eq!(state.tool_signals.subscribers(), 1);
}

#[test]
fn dropping_the_source_unregisters_the_listener() {
    let state = state();
    let response = intercept(&state, &request(&listen_frame(&json!(9), true)))
        .expect("a listen request is intercepted");
    let stream = streamed(response);
    assert_eq!(state.tool_signals.subscribers(), 1);
    drop(stream);
    assert_eq!(state.tool_signals.subscribers(), 0);
    assert!(state.tool_signals.publish("view-two"));
    assert_eq!(state.tool_signals.subscribers(), 0, "nobody is woken");
}

#[test]
fn a_cursor_minted_before_a_view_change_is_refused() {
    let state = state();
    let page = crate::protocol::McpHandler::tools_list_page(
        &state,
        Some(&json!({ "_meta": { (crate::tools_page::PAGE_SIZE_KEY): 1 } })),
    )
    .expect("the first page is served");
    let cursor = page["nextCursor"]
        .as_str()
        .expect("a small page carries a cursor")
        .to_owned();

    assert!(
        !state.announce_tool_view(),
        "the first announce only seeds the baseline"
    );
    {
        let mut config = state.config.lock().expect("config mutex");
        config.set_mcp_tool_groups(clio_types::ToolGroup::ALL.to_vec());
    }
    assert!(state.announce_tool_view(), "the view changed");

    let error =
        crate::protocol::McpHandler::tools_list_page(&state, Some(&json!({ "cursor": cursor })))
            .expect_err("a cursor from the old view is refused");
    assert_eq!(error.code, ERR_INVALID_PARAMS);
    assert!(
        error.message.contains("different tool view"),
        "{}",
        error.message
    );
}
