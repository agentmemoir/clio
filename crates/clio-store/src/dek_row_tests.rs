// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `dek_row.rs`: the composite row identity. Never
//! compiled into production builds.

use super::composite_row_id;

#[test]
fn composite_row_ids_keep_field_boundaries() {
    assert_ne!(
        composite_row_id(&["ab", "c"]),
        composite_row_id(&["a", "bc"])
    );
    assert_eq!(
        composite_row_id(&["bank", "key"]),
        "00000004bank00000003key"
    );
}
