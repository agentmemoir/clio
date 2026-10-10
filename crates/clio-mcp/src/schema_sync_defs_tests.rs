// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the sync tool definitions this file
//! publishes: the six names, their descriptions, the `sync_key` override on
//! `sync_status`, the absence of `sync_key` on the two transfer tools, and the
//! absence of the removed single `token` on `sync_serve`.
//!
//! ## Owns
//! - Every test of `sync_defs`
//!
//! ## Does not own
//! - The pack assembly or schema validation (see `schema_tests`)
//! - Tool dispatch (see `sync_tools_tests`)
//!
//! ## Boundary
//!
//! Changes to a sync tool's parameter contract should normally update tests
//! here. Changes to dispatch should not.
//!
//! Keep this module focused on the definitions.

use super::*;

#[test]
fn every_sync_tool_definition_is_named_and_described() {
    let names: Vec<&str> = sync_defs().iter().map(|def| def.name).collect();

    assert_eq!(
        names,
        vec![
            "sync_push",
            "sync_pull",
            "sync_status",
            "sync_key_status",
            "sync_serve",
            "sync_ack_skip",
        ]
    );
    for def in sync_defs() {
        assert!(
            !def.description.is_empty(),
            "{} has no description",
            def.name
        );
    }
}

/// Neither transfer tool declares a `sync_key` argument any more: the key comes
/// only from this node's recorded store, so a published schema that still
/// offered the argument would invite a call the handler now rejects.
#[test]
fn the_transfer_tools_declare_no_sync_key_argument() {
    for name in ["sync_push", "sync_pull"] {
        let def = sync_defs()
            .into_iter()
            .find(|def| def.name == name)
            .expect("a sync client tool");
        let props = def.to_tool()["inputSchema"]["properties"].clone();

        assert!(
            props.get("sync_key").is_none(),
            "{name} still declares sync_key: {props}"
        );
    }
}

/// `sync_serve` publishes no `token` argument: the single-token server input
/// was removed, so a schema that still offered it would invite a call the
/// handler rejects. Asserted by name — an index-based check would pass if the
/// property returned in another position.
#[test]
fn the_serve_tool_declares_no_legacy_token_argument() {
    let serve = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_serve")
        .expect("the sync server tool");
    let tool = serve.to_tool();
    let props = tool["inputSchema"]["properties"].clone();

    assert!(
        props.get("token").is_none(),
        "sync_serve still declares the removed token: {props}"
    );
    assert!(
        props.get("token_file").is_some(),
        "sync_serve must keep token_file as its credential source: {props}"
    );
    // Strict input validation is what makes the removal effective: with
    // `additionalProperties: false`, an old client that still sends `token`
    // is refused rather than silently bound without a credential.
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
}

/// Both serve descriptions point at the scoped record store only: they name
/// `clio auth token add` and never offer the removed single token as an
/// alternative credential source.
#[test]
fn the_serve_descriptions_advertise_only_the_scoped_credential_store() {
    let serve = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_serve")
        .expect("the sync server tool");
    let tool = serve.to_tool();
    let props = &tool["inputSchema"]["properties"];
    let descriptions = [
        (
            "sync_serve",
            tool["description"].as_str().expect("a serve description"),
        ),
        (
            "token_file",
            props["token_file"]["description"]
                .as_str()
                .expect("a token_file description"),
        ),
    ];

    for (owner, description) in descriptions {
        assert!(
            description.contains("clio auth token add"),
            "the {owner} description never names the scoped record store: {description}"
        );
        for legacy in [
            "Requires a bearer token",
            "Additive to `token`",
            "or `token`",
        ] {
            assert!(
                !description.contains(legacy),
                "the {owner} description still advertises the legacy single token \
                 (`{legacy}`): {description}"
            );
        }
    }
}

/// `sync_status` keeps its `sync_key` override while the transfer tools lost
/// theirs, so the two contracts cannot be read as the same shape.
#[test]
fn the_status_tool_keeps_its_sync_key_override_after_the_transfer_tools_lost_theirs() {
    let status = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_status")
        .expect("the status tool");
    assert!(
        status.to_tool()["inputSchema"]["properties"]
            .get("sync_key")
            .is_some(),
        "sync_status must still declare sync_key"
    );
}

/// `sync_key` accepts one string or an array of strings on `sync_status`, so an
/// integrator can express a rotation through the published schema instead of
/// guessing at the wire shape.
#[test]
fn the_sync_key_argument_accepts_one_string_or_an_array_on_the_status_tool() {
    let def = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_status")
        .expect("a sync tool carrying sync_key");
    let prop = def.to_tool()["inputSchema"]["properties"]["sync_key"].clone();

    // A plain `type` would pin one shape; the pair is the contract.
    assert!(prop.get("type").is_none(), "{prop}");
    let branches = prop["oneOf"].as_array().expect("oneOf branches");
    assert_eq!(branches.len(), 2, "{prop}");
    assert_eq!(branches[0]["type"], "string", "{prop}");
    assert_eq!(branches[1]["type"], "array", "{prop}");
    assert_eq!(branches[1]["items"]["type"], "string", "{prop}");
}

/// The status row declares the `sync_key` override its handler reads, so the
/// published contract cannot differ from the behaviour.
#[test]
fn the_status_row_declares_the_sync_key_override_the_handler_reads() {
    let def = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_status")
        .expect("the status row");
    let tool = def.to_tool();
    let description = tool["inputSchema"]["properties"]["sync_key"]["description"]
        .as_str()
        .expect("the sync_key override");

    assert!(
        description.contains("the keys the node itself holds"),
        "{description}"
    );
    assert!(description.contains("overrides"), "{description}");
    assert!(
        description.contains("Never returns key material"),
        "{description}"
    );
}

/// The key status row names the per-key list the response carries: each key's
/// fingerprint, creation time, whether it is current, and where it is held.
#[test]
fn the_key_status_description_names_the_per_key_list() {
    let def = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_key_status")
        .expect("the key status row");
    let tool = def.to_tool();
    let description = tool["description"].as_str().expect("a description");

    for fact in [
        "per-key list",
        "fingerprint",
        "creation time",
        "whether it is the current key",
        "the store that holds it",
    ] {
        assert!(
            description.contains(fact),
            "the description never names `{fact}`: {description}"
        );
    }
    assert!(
        !description.contains("retrievable"),
        "key bytes are never retrievable: {description}"
    );
}

#[test]
fn the_key_status_row_takes_no_arguments_and_never_returns_the_key() {
    let def = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_key_status")
        .expect("the key status row");

    assert!(def.required.is_empty(), "{:?}", def.required);
    assert!(def.props.is_empty(), "{:?}", def.props);
    let tool = def.to_tool();
    let description = tool["description"].as_str().expect("a description");
    assert!(
        description.contains("Never returns the key"),
        "{description}"
    );
}

/// The description must name the store literals the wire actually emits.
///
/// `KeySource::as_str` writes `file` or `keystore` and nothing else, so a
/// description that says "the OS secret store" leaves an integrator unable to
/// learn the token to compare against.
#[test]
fn the_key_status_description_names_the_store_literals_the_wire_emits() {
    let def = sync_defs()
        .into_iter()
        .find(|def| def.name == "sync_key_status")
        .expect("the key status row");
    let description = def.to_tool()["description"]
        .as_str()
        .expect("a description")
        .to_owned();

    for store in clio_config::KeySource::ALL {
        assert!(
            description.contains(store.as_str()),
            "the description never names the emitted store `{}`: {description}",
            store.as_str()
        );
    }
    assert!(
        !description.contains("file or the OS secret store"),
        "the wire never emits that phrase: {description}"
    );
    assert!(
        description.contains("null when it did not"),
        "a platform that keeps no creation time reads as null: {description}"
    );
}
