// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio status` report model: the healthy/degraded
//! verdict, JSON/text rendering for every Compose state, and the lenient
//! `docker compose ps` parser.
//!
//! ## Owns
//! - `assemble`, `to_json`, `render_text`, and `parse_compose_ps` tests
//!
//! ## Does not own
//! - The command surface and exit mapping (see `status_cli_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the report shape or rendering changes; they never change it.
//!
//! Keep this module focused on the status report model.

use super::*;
use clio_ops::Severity;
use serde_json::json;

fn setup() -> SetupView {
    SetupView {
        deployment_config: Some("/tmp/deployment.json".to_owned()),
        embed: Some(ServiceConf {
            provider: "tei".to_owned(),
            url: "http://127.0.0.1:34311".to_owned(),
            model: "m".to_owned(),
        }),
        rerank: None,
        secrets: BTreeMap::from([(
            "credentials.embed_api_key".to_owned(),
            "****cret".to_owned(),
        )]),
    }
}

fn database() -> DatabaseView {
    DatabaseView {
        backend: "sqlite".to_owned(),
        url: "/data/clio.db".to_owned(),
        encryption: "not encrypted, this machine only".to_owned(),
        key_location: None,
        key_error: None,
    }
}

fn diagnose(ok: bool, backend_reachable: bool) -> DiagnoseView {
    DiagnoseView {
        ok,
        backend_reachable,
        findings: if ok {
            vec![]
        } else {
            vec![OpsFinding::new(
                "BACKEND_UNREACHABLE",
                Severity::Error,
                "store",
                "boom",
            )]
        },
        error: None,
    }
}

#[test]
fn healthy_when_diagnose_ok_and_reachable() {
    let report = assemble(
        setup(),
        database(),
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    assert!(report.ok);
}

/// The report names the deeper check it does not run, in both the model and the
/// rendered text, so a healthy line is not read as a clean bill of health.
#[test]
fn report_points_at_the_verify_pass() {
    let report = assemble(
        setup(),
        database(),
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    assert_eq!(report.verify_pointer, clio_ops::VERIFY_POINTER);
    assert!(render_text(&report).contains("clio ops verify"));
}

#[test]
fn degraded_when_diagnose_not_ok() {
    let report = assemble(
        setup(),
        database(),
        vec![34300],
        ComposeView::NotMaterialized,
        diagnose(false, true),
    );
    assert!(!report.ok);
}

#[test]
fn degraded_when_backend_unreachable() {
    let report = assemble(
        setup(),
        database(),
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, false),
    );
    assert!(!report.ok);
}

#[test]
fn json_round_trips_the_report() {
    let report = assemble(
        setup(),
        database(),
        vec![34300, 34301],
        ComposeView::Available {
            services: vec![ServiceState {
                service: "postgres".to_owned(),
                state: "running".to_owned(),
            }],
        },
        diagnose(true, true),
    );
    let raw = to_json(&report);
    let value: Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(value["ok"], json!(true));
    assert_eq!(value["ports"], json!([34300, 34301]));
    assert_eq!(value["compose"]["state"], json!("available"));
    assert_eq!(
        value["compose"]["services"][0]["service"],
        json!("postgres")
    );
    assert_eq!(
        value["setup"]["secrets"]["credentials.embed_api_key"],
        json!("****cret")
    );
}

#[test]
fn json_reports_encryption_and_key_location_without_install_type() {
    let view = DatabaseView {
        backend: "postgres".to_owned(),
        url: "postgres://***@db.example.com:5432/clio".to_owned(),
        encryption: "encrypted, remote".to_owned(),
        key_location: Some("macOS Keychain".to_owned()),
        key_error: None,
    };
    let report = assemble(
        setup(),
        view,
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    let value: Value = serde_json::from_str(&to_json(&report)).expect("json");
    assert_eq!(value["database"]["backend"], json!("postgres"));
    assert_eq!(
        value["database"]["url"],
        json!("postgres://***@db.example.com:5432/clio")
    );
    assert_eq!(value["database"]["encryption"], json!("encrypted, remote"));
    assert_eq!(value["database"]["key_location"], json!("macOS Keychain"));
    assert!(
        value["database"].get("key_error").is_none(),
        "{}",
        value["database"]
    );
    assert!(
        value["setup"].get("install_type").is_none(),
        "the removed field must not be serialized: {}",
        value["setup"]
    );
}

#[test]
fn json_omits_key_fields_that_have_no_value() {
    let mut view = database();
    view.key_error = Some("secret store locked".to_owned());
    let report = assemble(
        setup(),
        view,
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    let value: Value = serde_json::from_str(&to_json(&report)).expect("json");
    assert!(
        value["database"].get("key_location").is_none(),
        "{}",
        value["database"]
    );
    assert_eq!(value["database"]["key_error"], json!("secret store locked"));
}

#[test]
fn text_render_shows_database_encryption_and_key_location() {
    let view = DatabaseView {
        backend: "postgres".to_owned(),
        url: "postgres://***@127.0.0.1:34310/clio".to_owned(),
        encryption: "encrypted: no, this machine only".to_owned(),
        key_location: Some("key file /keys/clio.key".to_owned()),
        key_error: None,
    };
    let report = assemble(
        setup(),
        view,
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    let text = render_text(&report);
    assert!(
        text.contains(
            "database: postgres postgres://***@127.0.0.1:34310/clio \
             (encrypted: no, this machine only)"
        ),
        "{text}"
    );
    assert!(
        text.contains("encryption key: key file /keys/clio.key"),
        "{text}"
    );
}

#[test]
fn text_render_reports_a_key_read_failure_and_says_nothing_without_a_key() {
    let mut view = database();
    view.key_error = Some("secret store locked".to_owned());
    let report = assemble(
        setup(),
        view,
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    let text = render_text(&report);
    assert!(
        text.contains("encryption key: could not be read (secret store locked)"),
        "{text}"
    );

    let plain = assemble(
        setup(),
        database(),
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    assert!(
        !render_text(&plain).contains("encryption key:"),
        "no key, no line"
    );
}

#[test]
fn text_render_reports_each_compose_state() {
    let available = assemble(
        setup(),
        database(),
        vec![34300],
        ComposeView::Available {
            services: vec![
                ServiceState {
                    service: "embed".to_owned(),
                    state: "running".to_owned(),
                },
                ServiceState {
                    service: "postgres".to_owned(),
                    state: "exited".to_owned(),
                },
            ],
        },
        diagnose(true, true),
    );
    let text = render_text(&available);
    assert!(text.contains("clio status: healthy"), "{text}");
    assert!(text.contains("ports:    34300"), "{text}");
    assert!(text.contains("embed=running, postgres=exited"), "{text}");
    assert!(
        text.contains("credentials.embed_api_key=****cret"),
        "{text}"
    );

    let none = assemble(
        setup(),
        database(),
        vec![],
        ComposeView::Available { services: vec![] },
        diagnose(true, true),
    );
    assert!(render_text(&none).contains("compose:  no services"));

    let unavailable = assemble(
        setup(),
        database(),
        vec![],
        ComposeView::Unavailable {
            reason: "no docker".to_owned(),
        },
        diagnose(false, false),
    );
    let text = render_text(&unavailable);
    assert!(text.contains("clio status: degraded"), "{text}");
    assert!(text.contains("compose:  unavailable: no docker"), "{text}");
    assert!(
        text.contains("diagnose: not ok (backend reachable: false)"),
        "{text}"
    );
    assert!(
        text.contains("[Error] BACKEND_UNREACHABLE store: boom"),
        "{text}"
    );

    let not_materialized = assemble(
        setup(),
        database(),
        vec![],
        ComposeView::NotMaterialized,
        diagnose(true, true),
    );
    assert!(render_text(&not_materialized).contains("compose:  not materialized"));
}

#[test]
fn text_render_shows_unset_services_and_error() {
    let mut view = setup();
    view.embed = None;
    view.rerank = None;
    view.secrets = BTreeMap::new();
    let mut diag = diagnose(false, false);
    diag.error = Some("open failed".to_owned());
    let report = assemble(view, database(), vec![], ComposeView::NotMaterialized, diag);
    let text = render_text(&report);
    assert!(text.contains("embed=unset rerank=unset"), "{text}");
    assert!(text.contains("error: open failed"), "{text}");
    assert!(!text.contains("secrets:"), "{text}");
}

#[test]
fn parse_compose_ps_reads_array_single_and_ndjson() {
    let array =
        r#"[{"Service":"postgres","State":"running"},{"Service":"embed","State":"exited"}]"#;
    assert_eq!(
        parse_compose_ps(array),
        vec![
            ServiceState {
                service: "embed".to_owned(),
                state: "exited".to_owned()
            },
            ServiceState {
                service: "postgres".to_owned(),
                state: "running".to_owned()
            },
        ],
        "sorted by service"
    );

    let single = r#"{"Service":"postgres","State":"running"}"#;
    assert_eq!(
        parse_compose_ps(single),
        vec![ServiceState {
            service: "postgres".to_owned(),
            state: "running".to_owned()
        }]
    );

    let ndjson = "{\"Service\":\"postgres\",\"State\":\"running\"}\n{\"Service\":\"rerank\",\"Status\":\"Up\"}";
    assert_eq!(
        parse_compose_ps(ndjson),
        vec![
            ServiceState {
                service: "postgres".to_owned(),
                state: "running".to_owned()
            },
            ServiceState {
                service: "rerank".to_owned(),
                state: "Up".to_owned()
            },
        ]
    );
}

#[test]
fn parse_compose_ps_is_lenient() {
    let blank = parse_compose_ps("   ");
    assert!(blank.is_empty(), "{blank:?}");
    let unparsable = parse_compose_ps("not json");
    assert!(unparsable.is_empty(), "{unparsable:?}");
    assert!(
        parse_compose_ps(r#"[{"State":"running"}]"#).is_empty(),
        "no service name"
    );
    assert_eq!(
        parse_compose_ps(r#"[{"Name":"pg","State":"running"}]"#),
        vec![ServiceState {
            service: "pg".to_owned(),
            state: "running".to_owned()
        }],
        "Name is a fallback key"
    );
    assert_eq!(
        parse_compose_ps(r#"[{"Service":"pg"}]"#),
        vec![ServiceState {
            service: "pg".to_owned(),
            state: "unknown".to_owned()
        }],
        "missing state defaults to unknown"
    );
}
