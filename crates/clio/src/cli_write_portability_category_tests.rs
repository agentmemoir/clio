// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Regression test for an invalid export category being silently accepted.

use super::PortabilityWriteGroup;
use crate::cli_write_group::WriteGroup;

#[test]
fn export_rejects_an_unknown_category_before_dispatch() {
    let spec = PortabilityWriteGroup.spec("export");
    let parsed = crate::cli_args::parse(
        &spec,
        [
            "--out=bundle.json".to_owned(),
            "--category=bogus".to_owned(),
        ],
    )
    .expect("parse flags");
    let Err(error) = PortabilityWriteGroup.build("export", &parsed) else {
        panic!("unknown category must fail at the CLI boundary")
    };
    assert!(error.message().contains("category"), "{}", error.message());
}
