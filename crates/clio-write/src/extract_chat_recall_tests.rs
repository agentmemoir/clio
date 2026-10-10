// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module owns deterministic regression coverage for hosted abstract-schema
//! extraction recall and the shared Rust verifier parity contract.
//!
//! ## Owns
//! - A transport-backed provider response through `ChatExtractor`, span
//!   verification, and `extract_verify_store` for previously empty labels
//! - Runtime evaluation of every case in `scripts/verifier_parity.json` through
//!   the production `verify_snapshot` entry point
//!
//! ## Does not own
//! - Production transport, response parsing, span matching, or admission logic
//! - The Python verifier implementation or its cross-language harness
//!
//! ## Boundary
//!
//! Changes to observable extraction/admission behavior or the shared verifier
//! contract should normally require changes here. Production rule changes belong
//! in their owning modules.
//!
//! Keep this module focused on deterministic extraction and verifier regressions.

use super::{ChatExtractor, SYSTEM_PROMPT, build_user_message};
use crate::extract::{ExtractRequest, Transport};
use crate::pipeline::{ExtractStoreArgs, extract_verify_store};
use crate::schema::{ExtractiveKind, FieldSpec};
use crate::store_path::VerifiedStoreRequest;
use crate::telemetry::VerifyVecSink;
use crate::verify::{VerifyConfig, nfc, verify_snapshot};
use clio_admission::decision::Candidate;
use clio_admission::gated::SignalSource;
use clio_admission::{AdmissionPolicy, RankingEnv, VecSink};
use clio_types::{
    Actor, AmError, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

#[derive(Debug)]
struct ProviderCall {
    url: String,
    body: Value,
    bearer: Option<String>,
}

#[derive(Debug)]
struct MockProviderTransport {
    response: Value,
    calls: Mutex<Vec<ProviderCall>>,
}

impl MockProviderTransport {
    fn new(response: Value) -> Self {
        Self {
            response,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl Transport for MockProviderTransport {
    fn post_json(&self, url: &str, body: &Value) -> Result<Value, AmError> {
        self.post_json_bearer(url, body, None)
    }

    fn post_json_bearer(
        &self,
        url: &str,
        body: &Value,
        bearer: Option<&str>,
    ) -> Result<Value, AmError> {
        self.calls
            .lock()
            .expect("provider calls lock")
            .push(ProviderCall {
                url: url.into(),
                body: body.clone(),
                bearer: bearer.map(str::to_owned),
            });
        Ok(self.response.clone())
    }
}

#[derive(Debug, Default)]
struct EmptySignals;

impl SignalSource for EmptySignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, AmError> {
        Ok(vec![])
    }
}

fn base_item() -> MemoryItem {
    MemoryItem {
        id: "extract-recall".into(),
        bank_id: "bank-1".into(),
        subject_id: "subject-1".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: None,
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

fn store_parts<'a>(
    ctx: &'a OpContext,
    env: &'a RankingEnv,
    admit: &'a VecSink,
    verify_sink: &'a VerifyVecSink,
    fields: &'a [FieldSpec],
    verify: &VerifyConfig,
) -> VerifiedStoreRequest<'a> {
    VerifiedStoreRequest {
        item: base_item(),
        ctx,
        source_text: None,
        evidence_ref: None,
        fields,
        verify: verify.clone(),
        policy: AdmissionPolicy::new(env),
        admit_sink: admit,
        verify_sink,
        timestamp: "t0",
    }
}

#[test]
fn abstract_version_label_is_sent_with_its_source_fact() {
    let request = ExtractRequest {
        source_text: "CI uses Rust 1.75.0 on ubuntu-22.04 for the clio build.".into(),
        tool_payload: None,
        schema_template: json!({"rust_version": "", "os": ""}),
        verify_feedback: None,
        context: None,
    };
    let message = build_user_message(&request);
    assert!(message.contains("\"rust_version\""), "{message}");
    assert!(message.contains("Rust 1.75.0"), "{message}");
    assert!(
        SYSTEM_PROMPT.contains("Populate every schema field"),
        "{SYSTEM_PROMPT}"
    );
}

fn assert_mock_provider_request(
    extractor: &ChatExtractor<MockProviderTransport>,
    schema_template: &Value,
    source_text: &str,
) {
    let calls = extractor
        .transport
        .calls
        .lock()
        .expect("provider calls lock");
    assert_eq!(calls.len(), 1, "one mock response should be consumed");
    let call = &calls[0];
    assert_eq!(call.url, "https://provider.example/v1/chat/completions");
    assert_eq!(call.bearer.as_deref(), Some("test-token"));
    assert_eq!(call.body["model"], "mock-model");
    assert_eq!(call.body["temperature"], 0.0);
    assert_eq!(call.body["messages"][0]["role"], "system");
    assert_eq!(call.body["messages"][0]["content"], SYSTEM_PROMPT);
    assert_eq!(call.body["messages"][1]["role"], "user");
    let user = call.body["messages"][1]["content"]
        .as_str()
        .expect("provider user message");
    assert!(user.contains(&schema_template.to_string()), "{user}");
    assert!(user.contains(source_text), "{user}");
}

#[test]
fn abstract_labels_flow_through_mock_provider_into_admitted_snapshot() {
    let source_text = "CI uses Rust 1.75.0 on ubuntu-22.04 for the clio build.";
    let schema_template = json!({"rust_version": "", "os": ""});
    let expected_snapshot = json!({"rust_version": "Rust 1.75.0", "os": "ubuntu-22.04"});
    let provider_content = json!({
        "snapshot": expected_snapshot,
        "gist": "The build environment versions."
    });
    let provider_response = json!({
        "choices": [{"message": {"role": "assistant", "content": provider_content.to_string()}}]
    });
    let transport = MockProviderTransport::new(provider_response);
    let extractor = ChatExtractor::new(
        "https://provider.example",
        "mock-model",
        0.0,
        Some("test-token".into()),
        transport,
    )
    .expect("mock extractor");
    let fields = vec![
        FieldSpec::new("rust_version", ExtractiveKind::Entity, true),
        FieldSpec::new("os", ExtractiveKind::Entity, true),
    ];
    let verify = VerifyConfig::default();
    let ctx = OpContext::new(Bank::new("bank-1"), Actor::Agent);
    let env = RankingEnv::system_defaults();
    let admit = VecSink::default();
    let verify_sink = VerifyVecSink::default();
    let mut store = store_parts(&ctx, &env, &admit, &verify_sink, &fields, &verify);
    let signals = EmptySignals;
    let mut writes = Vec::new();

    let outcome = extract_verify_store(
        ExtractStoreArgs {
            extractor: &extractor,
            source_text,
            schema_template: schema_template.clone(),
            tool_payload: None,
            fields: &fields,
            verify: &verify,
            item_base: base_item(),
            candidate: Candidate::Semantic(SemanticCategory::Persona),
            signal_source: &signals,
        },
        &mut store,
        &mut |item| {
            writes.push(item.clone());
            Ok(())
        },
    )
    .expect("extract, verify, and store");

    assert!(outcome.decision.pass, "{:?}", outcome.decision);
    assert_eq!(outcome.extract_calls, 1);
    assert!(outcome.retained.is_none());
    assert_eq!(writes.len(), 1, "admitted snapshot must be written once");

    assert_mock_provider_request(&extractor, &schema_template, source_text);

    let stored = &writes[0];
    let snapshot = stored.snapshot.as_ref().expect("stored candidate snapshot");
    assert!(
        snapshot
            .as_object()
            .is_some_and(|candidate| !candidate.is_empty()),
        "candidate must be non-empty: {snapshot}"
    );
    assert_eq!(snapshot, &expected_snapshot);
    assert_eq!(
        stored.gist.as_deref(),
        Some("The build environment versions.")
    );
    assert_eq!(stored.source_ref.as_deref(), Some("extract"));

    let report = verify_snapshot(source_text, snapshot, &fields, &verify);
    assert!(report.ok, "stored candidate must verify: {report:?}");
    assert_eq!(report.spans.len(), fields.len());
    let normalized_source = nfc(source_text);
    for field in ["rust_version", "os"] {
        let span = report
            .spans
            .iter()
            .find(|span| span.field == field)
            .unwrap_or_else(|| panic!("missing span for {field}: {report:?}"));
        assert!(span.start < span.end, "empty span for {field}: {span:?}");
        let evidence = normalized_source
            .get(span.start..span.end)
            .expect("span must use valid NFC character offsets");
        assert_eq!(evidence, snapshot[field].as_str().expect("string field"));
    }
}

#[derive(Debug, Deserialize)]
struct ParityTable {
    format: String,
    cases: Vec<ParityCase>,
}

#[derive(Debug, Deserialize)]
struct ParityCase {
    id: String,
    source_text: String,
    snapshot: Value,
    fields: Vec<ParityField>,
    verify: ParityVerify,
    expected_ok: bool,
}

#[derive(Debug, Deserialize)]
struct ParityField {
    path: String,
    kind: String,
    required: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ParityVerify {
    day_first: Option<bool>,
    number_tolerance: Option<f64>,
    comma_grouping: Option<bool>,
}

fn parity_kind(raw: &str) -> ExtractiveKind {
    match raw {
        "entity" => ExtractiveKind::Entity,
        "number" => ExtractiveKind::Number,
        "date" => ExtractiveKind::Date,
        other => panic!("unsupported verifier parity field kind: {other}"),
    }
}

#[test]
fn public_verifier_parity_table_matches_rust_verifier() {
    let table: ParityTable =
        serde_json::from_str(include_str!("../../../scripts/verifier_parity.json"))
            .expect("parse public verifier parity table");
    assert_eq!(table.format, "clio-verifier-parity-v1");
    assert!(!table.cases.is_empty(), "parity table must contain cases");

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/extract_quality.py");
    let python_output = Command::new("python3")
        .arg(&script)
        .arg("--parity")
        .output()
        .expect("run the shared Python parity check");
    assert!(
        python_output.status.success(),
        "Python parity command failed: {}",
        String::from_utf8_lossy(&python_output.stderr)
    );
    let python_report: Value =
        serde_json::from_slice(&python_output.stdout).expect("parse Python parity JSON");
    let python_cases = python_report["cases"]
        .as_array()
        .expect("Python parity cases");
    assert_eq!(python_cases.len(), table.cases.len());

    let mut mismatches = Vec::new();
    let mut rust_results = Vec::new();
    for case in table.cases {
        let ParityCase {
            id,
            source_text,
            snapshot,
            fields: raw_fields,
            verify: overrides,
            expected_ok,
        } = case;
        let fields: Vec<FieldSpec> = raw_fields
            .iter()
            .map(|field| {
                FieldSpec::new(field.path.clone(), parity_kind(&field.kind), field.required)
            })
            .collect();
        let defaults = VerifyConfig::default();
        let verify = VerifyConfig {
            day_first: overrides.day_first,
            number_tolerance: overrides
                .number_tolerance
                .unwrap_or(defaults.number_tolerance),
            comma_grouping: overrides.comma_grouping.unwrap_or(defaults.comma_grouping),
        };
        let report = verify_snapshot(&source_text, &snapshot, &fields, &verify);
        rust_results.push((id.clone(), report.ok));
        if report.ok != expected_ok {
            mismatches.push(format!(
                "{id}: expected_ok={expected_ok}, actual={} ({report:?})",
                report.ok
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "shared verifier parity failures:\n{}",
        mismatches.join("\n")
    );
    for ((rust_id, rust_ok), python_case) in rust_results.iter().zip(python_cases) {
        assert_eq!(rust_id, python_case["id"].as_str().expect("Python case id"));
        assert_eq!(
            *rust_ok,
            python_case["actual_ok"]
                .as_bool()
                .expect("Python actual_ok")
        );
    }
}
