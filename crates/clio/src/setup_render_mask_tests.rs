// Copyright 2026 Agent Memoir (https://agentmemour.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the masked reader seam: that a
//! typed secret never reaches the writer, and that the seam can be scripted so no
//! test opens a terminal.
//!
//! ## Owns
//! - Masked-reader behavior under test for `setup_render` and `setup_secrets`
//!
//! ## Does not own
//! - Any production behavior
//! - The render tests that do not involve a secret (see `setup_render_tests`)
//!
//! ## Boundary
//!
//! Changes to how a secret is read should normally update tests here.
//! Changes to a non-secret prompt should not.
//!
//! Keep this module focused on the secret-reading seam.

use super::*;
use crate::setup_secrets::SecretReader;
use crate::setup_test_seam::scripted_secrets;

/// A reader that records every label it was asked for.
///
/// Recording the label is what proves the wizard says which secret it wants, so a
/// test can assert the database password and each API key are read at their own
/// prompt rather than one field serving both.
#[derive(Default)]
struct Recorder {
    labels: Vec<String>,
    values: Vec<String>,
}

impl Recorder {
    fn reader(&mut self) -> Box<SecretReader> {
        let labels = std::mem::take(&mut self.labels);
        let values = std::mem::take(&mut self.values);
        let mut cursor = 0;
        let mut seen = labels;
        Box::new(move |_out: &mut dyn std::io::Write, label: &str| {
            seen.push(label.to_owned());
            let value = values
                .get(cursor)
                .cloned()
                .ok_or_else(|| format!("no scripted value for `{label}`"))?;
            cursor += 1;
            Ok(value)
        })
    }
}

#[test]
fn the_typed_secret_never_appears_in_the_writers_bytes() {
    let mut out: Vec<u8> = Vec::new();
    let secret = "correct horse battery staple";
    let mut reader = scripted_secrets(vec![secret.to_owned()]);
    let value = ask_masked(&mut out, "Password", &mut *reader).expect("a value");
    assert_eq!(value, secret);
    let printed = String::from_utf8(out).expect("utf8");
    assert!(!printed.contains(secret), "the secret leaked: {printed}");
    assert!(printed.contains("Password:"), "{printed}");
}

#[test]
fn the_prompt_names_the_secret_it_wants() {
    let mut out: Vec<u8> = Vec::new();
    let mut reader = Recorder {
        values: vec!["one".to_owned(), "two".to_owned()],
        ..Recorder::default()
    };
    let mut boxed = reader.reader();
    ask_masked(&mut out, "Password", &mut *boxed).expect("password");
    ask_masked(&mut out, "Embedding API key", &mut *boxed).expect("key");
    let printed = String::from_utf8(out).expect("utf8");
    assert!(printed.contains("Password:"), "{printed}");
    assert!(printed.contains("Embedding API key:"), "{printed}");
    assert!(!printed.contains("one"), "{printed}");
    assert!(!printed.contains("two"), "{printed}");
}

#[test]
fn a_reader_that_refuses_produces_a_refusal_naming_the_prompt() {
    let mut out: Vec<u8> = Vec::new();
    let mut reader: Box<SecretReader> = Box::new(|_out: &mut dyn std::io::Write, _label: &str| {
        Err("no terminal attached".to_owned())
    });
    let error = ask_masked(&mut out, "Password", &mut *reader).expect_err("a refusal");
    assert_eq!(
        error,
        Signal::Refused("Password: no terminal attached".to_owned())
    );
}

#[test]
fn a_secret_field_takes_any_word_as_the_secret() {
    // `b` and `q` are no longer ways out of anything, and a secret is the one field
    // where that matters most: a base64 key or a passphrase fragment can be one
    // character, so a reserved word here would silently corrupt a real key. A
    // terminal field gets its way back from `Esc`, which the prompt library
    // delivers; a reader cannot, so this one has no way back and says so by
    // returning the value.
    for typed in ["b", "q"] {
        let mut out: Vec<u8> = Vec::new();
        let mut reader = scripted_secrets(vec![typed.to_owned()]);
        let value = ask_masked(&mut out, "Password", &mut *reader).expect("a secret");
        assert_eq!(value, typed);
        let printed = String::from_utf8(out).expect("utf8");
        assert!(!printed.contains(typed), "the secret leaked: {printed}");
    }
}

#[test]
fn an_empty_scripted_reader_refuses_rather_than_returning_nothing() {
    let mut out: Vec<u8> = Vec::new();
    let mut reader = scripted_secrets(Vec::new());
    let error = ask_masked(&mut out, "Password", &mut *reader).expect_err("a refusal");
    assert_eq!(
        error,
        Signal::Refused("Password: the scripted reader had no value left to give".to_owned())
    );
}

#[test]
fn the_last_scripted_value_repeats_so_a_re_ask_is_covered() {
    let mut reader = scripted_secrets(vec!["first".to_owned()]);
    let mut writer: Vec<u8> = Vec::new();
    assert_eq!(reader(&mut writer, "Password").expect("one"), "first");
    assert_eq!(reader(&mut writer, "Password").expect("two"), "first");
}
