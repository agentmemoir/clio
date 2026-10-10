// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the compose tests of the pack fidelity
//! channel: which per-hit form each channel contributes, the budget fallback
//! to the marked reference, and the unchanged `flow` pack.
//!
//! ## Owns
//! - Pack-level assertions for the `exact` channel: the rendering, the gist,
//!   the marked reference, and the rendering that does not fit the budget
//! - The fidelity channel's resolution on the compose arguments: an explicit
//!   value, else the bank default, else `flow`, and a value naming neither
//!   refused as `invalid_argument`
//!
//! ## Does not own
//! - The selector's own branches and the tally fold (`pack_forms_tests`), and
//!   the dual budget and persona channel (`compose_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::compose::{ComposePack, ComposeRequest, Composer, PACK_OVERHEAD_TOKENS, PackSection};
use crate::fixtures_tests as fx;
use crate::pack_forms::{FormTally, HitForm, REFERENCE_MARKER};
use crate::render::render_exact;
use crate::surface::{ReadPolicyResolver, compose_from_json_with_policy};
use crate::token::estimate_tokens;
use crate::types::RetrieveRequest;
use clio_config::retention::RetentionProfile;
use clio_config::retention_levels::PackFidelityDefault;
use serde_json::{Value, json};
use std::sync::Arc;

fn composer(store: &Arc<clio_store::SqliteStore>) -> Composer {
    Composer::new(Arc::new(fx::retriever(store)))
}

fn section<'a>(pack: &'a ComposePack, name: &str) -> &'a PackSection {
    pack.sections.iter().find(|s| s.name == name).expect(name)
}

fn request(budget: usize, fidelity: PackFidelityDefault) -> ComposeRequest {
    ComposeRequest {
        bank: "bank".into(),
        query: Some("shared".into()),
        budget_tokens: budget,
        fidelity,
        ..ComposeRequest::new("bank")
    }
}

/// The stored snapshot whose rendering is large enough that a modest memory
/// budget cannot hold it, while the fixed marker plus the id costs four
/// tokens.
fn wide_snapshot() -> Value {
    json!({ "k": "a".repeat(240) })
}

#[test]
fn exact_channel_contributes_the_rendering_and_the_tally_counts_it() {
    let store = fx::store();
    let item = fx::item("A", "bank", "shared gist");
    fx::put_lexical(&store, &item, "shared");
    let render = render_exact(item.snapshot.as_ref().expect("snapshot")).expect("render");

    let pack = composer(&store)
        .compose(&request(400, PackFidelityDefault::Exact), None)
        .expect("compose");
    let memory = section(&pack, "memory");
    assert_eq!(memory.hit_forms, [HitForm::Exact], "{memory:?}");
    assert_eq!(memory.text, render, "the rendering wins over the gist");
    assert_eq!(pack.form_tally.exact, 1, "{:?}", pack.form_tally);
    assert_eq!(pack.form_tally.prose, 0, "{:?}", pack.form_tally);
}

#[test]
fn exact_channel_contributes_the_gist_without_a_rendering() {
    let store = fx::store();
    let mut item = fx::item("A", "bank", "shared gist");
    // No snapshot, so the hit carries no rendering to prefer.
    item.snapshot = None;
    fx::put_lexical(&store, &item, "shared");

    let pack = composer(&store)
        .compose(&request(400, PackFidelityDefault::Exact), None)
        .expect("compose");
    let memory = section(&pack, "memory");
    assert_eq!(memory.hit_forms, [HitForm::Prose], "{memory:?}");
    assert_eq!(memory.text, "shared gist", "{memory:?}");
    assert_eq!(
        pack.form_tally,
        FormTally {
            prose: 1,
            exact: 0,
            reference: 0
        }
    );
}

#[test]
fn exact_channel_contributes_a_marked_reference_without_either_half() {
    let store = fx::store();
    let mut item = fx::item("A", "bank", "shared");
    item.gist = None;
    item.snapshot = None;
    fx::put_lexical(&store, &item, "shared");

    let pack = composer(&store)
        .compose(&request(400, PackFidelityDefault::Exact), None)
        .expect("compose");
    let memory = section(&pack, "memory");
    assert_eq!(memory.hit_forms, [HitForm::Reference], "{memory:?}");
    assert_eq!(memory.text, format!("{REFERENCE_MARKER}A"), "{memory:?}");
    assert_eq!(
        pack.form_tally,
        FormTally {
            prose: 0,
            exact: 0,
            reference: 1
        }
    );
}

#[test]
fn a_rendering_over_the_remaining_budget_falls_to_the_marked_reference() {
    let store = fx::store();
    let mut item = fx::item("A", "bank", "shared");
    item.snapshot = Some(wide_snapshot());
    fx::put_lexical(&store, &item, "shared");
    let render = render_exact(item.snapshot.as_ref().expect("snapshot")).expect("render");
    let reference = estimate_tokens(&format!("{REFERENCE_MARKER}A"));
    assert!(
        estimate_tokens(&render) > reference,
        "the fixture must make the two costs differ"
    );
    // One token short of the rendering: the rendering is dropped whole and the
    // hit still fits as a marked reference.
    let budget = PACK_OVERHEAD_TOKENS + estimate_tokens(&render) - 1;

    let pack = composer(&store)
        .compose(&request(budget, PackFidelityDefault::Exact), None)
        .expect("compose");
    let memory = section(&pack, "memory");
    assert_eq!(memory.hit_forms, [HitForm::Reference], "{memory:?}");
    assert_eq!(memory.text, format!("{REFERENCE_MARKER}A"), "{memory:?}");
    assert_eq!(
        pack.form_tally,
        FormTally {
            prose: 0,
            exact: 0,
            reference: 1
        }
    );
    assert!(
        pack.truncated_ids.is_empty(),
        "the hit fits as a reference: {:?}",
        pack.truncated_ids
    );
    assert!(
        pack.total_tokens <= budget,
        "budget {budget} vs total {}",
        pack.total_tokens
    );
    assert_eq!(pack.total_tokens, PACK_OVERHEAD_TOKENS + reference);
    assert_ne!(memory.text, render, "the rendering is never shortened");
}

/// The budget a rendering is measured against is what is left at that hit's
/// turn, not the whole memory budget: with room for one rendering plus one
/// marked reference the first hit takes its rendering and the second, at the
/// next turn, no longer has room, so it falls back. The same hits on the `flow`
/// channel take their gists, because a rendering never wins there.
#[test]
fn the_budget_a_rendering_is_measured_against_is_what_is_left_at_that_turn() {
    let store = fx::store();
    for (id, gist) in [("A", "shared alpha"), ("B", "shared beta")] {
        let mut item = fx::item(id, "bank", gist);
        item.snapshot = Some(wide_snapshot());
        fx::put_lexical(&store, &item, gist);
    }
    let hits = fx::retriever(&store)
        .retrieve(&RetrieveRequest {
            include_exact: true,
            ..RetrieveRequest::new("bank", "shared")
        })
        .expect("retrieve")
        .hits;
    assert_eq!(hits.len(), 2, "both hits carry a rendering");
    let render = render_exact(&wide_snapshot()).expect("render");
    let reference = estimate_tokens(&format!("{REFERENCE_MARKER}A"));
    let budget = estimate_tokens(&render) + reference;

    let packed = super::pack_memory(&hits, budget, PackFidelityDefault::Exact);
    assert_eq!(
        FormTally::fold(packed.section.hit_forms.iter().copied()),
        FormTally {
            prose: 0,
            exact: 1,
            reference: 1
        }
    );
    let lines: Vec<&str> = packed.section.text.lines().collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], render, "the first hit took its rendering");
    assert_eq!(
        lines[1],
        format!("{REFERENCE_MARKER}{}", packed.section.item_ids[1]),
        "the second hit fell back to the marked reference"
    );
    assert_eq!(packed.tokens, budget, "the pack spends at most its budget");

    let flow = super::pack_memory(&hits, budget, PackFidelityDefault::Flow);
    assert_eq!(
        FormTally::fold(flow.section.hit_forms.iter().copied()),
        FormTally {
            prose: 2,
            exact: 0,
            reference: 0
        },
        "a rendering never wins on the flow channel"
    );
    assert_eq!(flow.section.text, "shared alpha\nshared beta");
    assert!(flow.tokens <= budget, "{}", flow.tokens);
}

#[test]
fn the_pack_boundary_sits_between_the_rendering_and_the_reference() {
    let store = fx::store();
    let mut item = fx::item("A", "bank", "shared");
    item.snapshot = Some(wide_snapshot());
    fx::put_lexical(&store, &item, "shared");
    let render = render_exact(item.snapshot.as_ref().expect("snapshot")).expect("render");
    let exact_budget = PACK_OVERHEAD_TOKENS + estimate_tokens(&render);

    let fits = composer(&store)
        .compose(&request(exact_budget, PackFidelityDefault::Exact), None)
        .expect("compose");
    assert_eq!(section(&fits, "memory").hit_forms, [HitForm::Exact]);
    assert_eq!(section(&fits, "memory").text, render);
    assert!(fits.total_tokens <= exact_budget, "{}", fits.total_tokens);

    let one_short = composer(&store)
        .compose(&request(exact_budget - 1, PackFidelityDefault::Exact), None)
        .expect("compose");
    assert_eq!(
        section(&one_short, "memory").hit_forms,
        [HitForm::Reference]
    );
    assert_eq!(
        section(&one_short, "memory").text,
        format!("{REFERENCE_MARKER}A")
    );
    assert!(
        one_short.total_tokens < exact_budget,
        "{}",
        one_short.total_tokens
    );
}

/// Binding stand-in whose bank default is the `exact` channel and whose other
/// read-side fields stay at the builtin default.
struct ExactBank;

impl ReadPolicyResolver for ExactBank {
    fn read_policy(&self, _bank: &str) -> RetentionProfile {
        RetentionProfile {
            pack_fidelity_default: PackFidelityDefault::Exact,
            ..RetentionProfile::default()
        }
    }
}

#[test]
fn the_bank_default_selects_the_exact_channel_and_an_explicit_value_wins() {
    let store = fx::store();
    let item = fx::item("A", "bank", "shared gist");
    fx::put_lexical(&store, &item, "shared");
    let composer = composer(&store);
    let args = json!({"bank": "bank", "query": "shared", "budget_tokens": 400});

    let from_bank = compose_from_json_with_policy(&composer, &args, None, Some(&ExactBank));
    let value = serde_json::to_value(&from_bank).expect("serialize");
    assert_eq!(
        value["sections"][1]["hit_forms"],
        json!(["exact"]),
        "{value}"
    );

    let explicit = compose_from_json_with_policy(
        &composer,
        &json!({
            "bank": "bank",
            "query": "shared",
            "budget_tokens": 400,
            "fidelity": "flow"
        }),
        None,
        Some(&ExactBank),
    );
    let value = serde_json::to_value(&explicit).expect("serialize");
    assert_eq!(
        value["sections"][1]["hit_forms"],
        json!(["prose"]),
        "{value}"
    );
}

/// An empty channel names no channel, so it is refused like any other unnamed
/// value instead of being read as absence and packed with the bank default. The
/// in-process surface refuses it exactly as the published MCP binding does, so a
/// mistyped channel never returns a different channel than the caller asked for.
#[test]
fn an_empty_fidelity_argument_is_refused_instead_of_taking_the_bank_default() {
    let store = fx::store();
    let composer = composer(&store);
    let empty = compose_from_json_with_policy(
        &composer,
        &json!({
            "bank": "bank",
            "query": "shared",
            "budget_tokens": 400,
            "fidelity": ""
        }),
        None,
        Some(&ExactBank),
    );
    let value = serde_json::to_value(&empty).expect("serialize");
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["code"], "invalid_argument", "{value}");
}

#[test]
fn an_unknown_channel_is_rejected_instead_of_silently_packing_in_flow() {
    let store = fx::store();
    let composer = composer(&store);
    let bad = compose_from_json_with_policy(
        &composer,
        &json!({"bank": "bank", "query": "shared", "fidelity": "lossy"}),
        None,
        None,
    );
    let value = serde_json::to_value(&bad).expect("serialize");
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["code"], "invalid_argument", "{value}");
}

/// The `flow` channel is the pre-channel pack, byte for byte. The same input is
/// composed twice — once naming the channel and once through the arguments that
/// carry no channel at all — and both must equal the frozen serialization, so
/// any change to `flow`'s output fails here.
#[test]
fn the_flow_channel_pack_is_byte_identical_to_the_channel_less_pack() {
    let store = fx::store();
    let item = fx::item("A", "bank", "shared gist");
    fx::put_lexical(&store, &item, "shared");
    let composer = composer(&store);
    let tokens = estimate_tokens("shared gist");

    let implicit = compose_from_json_with_policy(
        &composer,
        &json!({"bank": "bank", "query": "shared", "budget_tokens": 400}),
        None,
        None,
    );
    let named = compose_from_json_with_policy(
        &composer,
        &json!({
            "bank": "bank",
            "query": "shared",
            "budget_tokens": 400,
            "fidelity": "flow"
        }),
        None,
        None,
    );
    let implicit = serde_json::to_string(&implicit).expect("serialize");
    let named = serde_json::to_string(&named).expect("serialize");
    assert_eq!(implicit, named, "the channel must not change the flow pack");

    let expected = format!(
        r#"{{"ok":true,"sections":[{{"name":"persona","tokens":0,"item_ids":[],"text":"","truncated":false}},{{"name":"memory","tokens":{tokens},"item_ids":["A"],"hit_forms":["prose"],"text":"shared gist","truncated":false}}],"total_tokens":{},"budget_tokens":400,"persona_budget_tokens":392,"memory_budget_tokens":392,"truncated_ids":[],"retrieval_ran":true,"warnings":[],"form_tally":{{"prose":1,"exact":0,"reference":0}}}}"#,
        PACK_OVERHEAD_TOKENS + tokens
    );
    assert_eq!(implicit, expected, "the flow pack must not drift");
}
