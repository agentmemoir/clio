// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for pinning the Porter stemmer against the
//! reference vocabulary published with the algorithm, plus the correction
//! words the algorithm's `y` and doubled-consonant rules turn on.
//!
//! ## Owns
//! - The stemmer regression vectors, split into three tables so no test
//!   function is a wall of data
//!
//! ## Does not own
//! - The stemmer itself (`lexical_porter`)
//! - Token normalization or hashing (`lexical_digest`)
//!
//! ## Boundary
//!
//! Changes to stemming behavior should normally fail a vector here first.
//!
//! Keep this module focused on the reference vectors.

use super::stem;

/// Assert every `(word, stem)` pair in one table.
fn assert_matches(vectors: &[(&str, &str)]) {
    for (word, expected) in vectors {
        assert_eq!(&stem(word), expected, "stem({word})");
    }
}

/// Deterministic subset of the published vocabulary/output pairs, first half.
const SAMPLE_A: &[(&str, &str)] = &[
    ("a", "a"),
    ("acquainted", "acquaint"),
    ("affectionately", "affection"),
    ("almain", "almain"),
    ("anticipates", "anticip"),
    ("arinies", "arini"),
    ("atone", "aton"),
    ("bade", "bade"),
    ("bate", "bate"),
    ("beheaded", "behead"),
    ("betrayed", "betrai"),
    ("blinds", "blind"),
    ("bosom", "bosom"),
    ("bridges", "bridg"),
    ("burnt", "burnt"),
    ("canus", "canu"),
    ("cauf", "cauf"),
    ("charity", "chariti"),
    ("christ", "christ"),
    ("clogging", "clog"),
    ("comest", "comest"),
    ("conclude", "conclud"),
    ("conspire", "conspir"),
    ("cordis", "cordi"),
    ("cowards", "coward"),
    ("crusts", "crust"),
    ("dances", "danc"),
    ("deeps", "deep"),
    ("dere", "dere"),
    ("differences", "differ"),
    ("diseas", "disea"),
    ("distracts", "distract"),
    ("downward", "downward"),
    ("dungeons", "dungeon"),
    ("eliads", "eliad"),
    ("engaging", "engag"),
    ("equally", "equal"),
    ("eton", "eton"),
    ("expositor", "expositor"),
    ("fan", "fan"),
    ("fenny", "fenni"),
    ("flagons", "flagon"),
    ("footed", "foot"),
    ("fount", "fount"),
    ("function", "function"),
    ("gear", "gear"),
    ("glouceste", "gloucest"),
    ("grassy", "grassi"),
    ("guilts", "guilt"),
    ("harrows", "harrow"),
    ("helms", "helm"),
    ("holder", "holder"),
    ("hugger", "hugger"),
    ("immoderately", "immoder"),
    ("indeed", "inde"),
    ("inquir", "inquir"),
    ("inverness", "inver"),
    ("joints", "joint"),
    ("kites", "kite"),
    ("las", "la"),
];

/// Deterministic subset of the published vocabulary/output pairs, second half.
const SAMPLE_B: &[(&str, &str)] = &[
    ("leopards", "leopard"),
    ("litter", "litter"),
    ("luc", "luc"),
    ("male", "male"),
    ("mastcr", "mastcr"),
    ("mercury", "mercuri"),
    ("misbeliever", "misbeliev"),
    ("mons", "mon"),
    ("munition", "munit"),
    ("neeze", "neez"),
    ("noseless", "noseless"),
    ("oes", "oe"),
    ("ostler", "ostler"),
    ("pabylon", "pabylon"),
    ("parted", "part"),
    ("peize", "peiz"),
    ("perturbations", "perturb"),
    ("pirate", "pirat"),
    ("poesy", "poesi"),
    ("pouring", "pour"),
    ("pretend", "pretend"),
    ("prologue", "prologu"),
    ("pullet", "pullet"),
    ("quier", "quier"),
    ("raves", "rave"),
    ("reflex", "reflex"),
    ("replication", "replic"),
    ("revengement", "reveng"),
    ("robber", "robber"),
    ("rural", "rural"),
    ("saunder", "saunder"),
    ("scroop", "scroop"),
    ("seriously", "serious"),
    ("shifted", "shift"),
    ("signifies", "signifi"),
    ("sleeves", "sleev"),
    ("sola", "sola"),
    ("specialties", "specialti"),
    ("stablishment", "stablish"),
    ("stirring", "stir"),
    ("students", "student"),
    ("super", "super"),
    ("swelter", "swelter"),
    ("tastes", "tast"),
    ("thence", "thenc"),
    ("tide", "tide"),
    ("tours", "tour"),
    ("trimmed", "trim"),
    ("twilled", "twill"),
    ("uncropped", "uncrop"),
    ("unite", "unit"),
    ("unrivall", "unrival"),
    ("unwonted", "unwont"),
    ("varro", "varro"),
    ("villanous", "villan"),
    ("waits", "wait"),
    ("weavers", "weaver"),
    ("whoremaster", "whoremast"),
    ("wonder", "wonder"),
    ("yawn", "yawn"),
];

/// Words the `y` rule, the doubled-consonant rule, and the derivational steps
/// turn on; the algorithm's own corrections are visible here.
const CORRECTIONS: &[(&str, &str)] = &[
    ("ties", "ti"),
    ("cats", "cat"),
    ("feed", "feed"),
    ("agreed", "agre"),
    ("bled", "bled"),
    ("sing", "sing"),
    ("troubled", "troubl"),
    ("tanned", "tan"),
    ("falling", "fall"),
    ("hissing", "hiss"),
    ("failing", "fail"),
    ("happy", "happi"),
    ("sky", "sky"),
    ("rational", "ration"),
    ("hopeful", "hope"),
    ("goodness", "good"),
    ("allowance", "allow"),
    ("inference", "infer"),
    ("defensible", "defens"),
    ("dependent", "depend"),
    ("adoption", "adopt"),
    ("rate", "rate"),
    ("cease", "ceas"),
    ("controll", "control"),
    ("roll", "roll"),
    ("crying", "cry"),
    ("dyed", "dy"),
    ("dying", "dy"),
    ("buzzing", "buzz"),
    ("allayment", "allay"),
    ("annoyance", "annoy"),
    ("gypsy", "gypsi"),
    ("gayness", "gay"),
    ("employer", "employ"),
    ("employment", "employ"),
    ("enjoyer", "enjoy"),
    ("eye", "ey"),
    ("eyes", "ey"),
    ("flying", "fly"),
    ("destroyer", "destroy"),
    ("conveyance", "convey"),
    ("disloyal", "disloy"),
    ("bricklayer", "bricklay"),
    ("decayer", "decay"),
];

#[test]
fn reference_vocabulary_sample_first_half_matches() {
    assert_matches(SAMPLE_A);
}

#[test]
fn reference_vocabulary_sample_second_half_matches() {
    assert_matches(SAMPLE_B);
}

#[test]
fn correction_words_match() {
    assert_matches(CORRECTIONS);
}

#[test]
fn leaves_non_alphabetic_and_short_tokens_alone() {
    assert_eq!(stem("v2"), "v2");
    assert_eq!(stem("8080"), "8080");
    assert_eq!(stem("econnreset"), "econnreset");
    assert_eq!(stem("ab"), "ab");
    assert_eq!(stem(""), "");
}

/// The published reference vocabulary and its expected stems, committed as
/// fixtures so the full-vocabulary claim is reproducible from this repository.
/// Source: Martin Porter's official test data (`voc.txt` / `output.txt`,
/// 23 531 pairs, public domain).
const REFERENCE_VOCAB: &str = include_str!("../fixtures/porter_voc.txt");
const REFERENCE_STEMS: &str = include_str!("../fixtures/porter_output.txt");

/// Every published vocabulary word stems to its published output.
///
/// This is the full assertion behind the "0 mismatches against the published
/// 23 531-word reference vocabulary" evidence: the subset tables above pin the
/// interesting rules, this one pins all of them.
#[test]
fn full_published_vocabulary_matches() {
    let words: Vec<&str> = REFERENCE_VOCAB.lines().collect();
    let stems: Vec<&str> = REFERENCE_STEMS.lines().map(str::trim_end).collect();
    assert_eq!(words.len(), 23_531, "the committed vocabulary is complete");
    assert_eq!(words.len(), stems.len(), "vocabulary and output align");

    let mismatches: Vec<(&str, &str)> = words
        .into_iter()
        .zip(stems)
        .filter(|(word, expected)| stem(word) != *expected)
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} of 23 531 reference pairs mismatch; first few: {:?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(10)]
    );
}
