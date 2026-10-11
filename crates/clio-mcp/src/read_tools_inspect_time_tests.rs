// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Inclusive inspect upper bounds through the bound read dispatcher.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::json;

#[test]
fn inspect_includes_the_named_day_and_exact_timestamp_end() {
    let options = McpOpenOptions {
        default_bank: "inspect-time".into(),
        ..McpOpenOptions::default()
    };
    let state = McpState::open_with_effective(&options, &json!({})).expect("state");
    let stored = dispatch(
        &state,
        "store",
        &json!({
            "item": {
                "id": "boundary-item",
                "subject_id": "boundary-subject",
                "gist": "boundary record",
                "snapshot": {"entity": "boundary record"}
            },
            "category": "tool_config",
            "epistemic_kind": "fact",
            "source_text": "A boundary record was stored.",
            "timestamp": "2026-01-01T12:00:00Z"
        }),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");

    for time_end in [
        "2026-01-01",
        "2026-01-01T12:00:00Z",
        "2026-01-01T17:30:00+05:30",
    ] {
        let result = dispatch(
            &state,
            "inspect",
            &json!({"filter": {"time_end": time_end}}),
        );
        assert_eq!(result["ok"], json!(true), "{time_end}: {result}");
        assert_eq!(result["total"], json!(1), "{time_end}: {result}");
        assert_eq!(result["items"][0]["item_id"], json!("boundary-item"));
    }
}
