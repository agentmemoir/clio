// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the classic Porter (1980) English stemmer
//! that reduces one lexical token to its stem before the blind index hashes
//! it, so an inflected query still meets the stored form of its base word.
//!
//! ## Owns
//! - `stem`: the ASCII Porter stemmer (steps 1a through 5b)
//!
//! ## Does not own
//! - Token normalization, case folding, and diacritic folding (`lexical_digest`)
//! - Keyed hashing, the stored encoding, and the query encoding (`lexical_digest`)
//! - Per-backend SQL and tokenizer settings (`sqlite_index` / `postgres_index`)
//!
//! ## Boundary
//!
//! Changes to how a token is stemmed should normally require changes here.
//! Changes to what is stored or how it is matched should not require changes
//! here.
//!
//! The stemmer is applied identically on the write and the query path: both
//! sides call [`stem`] before hashing, so their stems always meet even where
//! this implementation differs in an edge case from a database tokenizer.
//!
//! Keep this module focused on the one stemming algorithm.

/// Longest word the algorithm looks at in one step is bounded by the suffix
/// tables below; short words are returned unchanged because no rule can fire.
const MIN_STEM_LEN: usize = 3;

/// Stem one normalized token.
///
/// Only lowercase ASCII alphabetic tokens are stemmed; every other token
/// (digits, mixed alphanumerics, non-ASCII text) is returned unchanged. The
/// function is total and deterministic: the same input always yields the same
/// output, with no allocation when no rule fires.
#[must_use]
pub(crate) fn stem(token: &str) -> String {
    if token.len() < MIN_STEM_LEN || !token.bytes().all(|b| b.is_ascii_alphabetic()) {
        return token.to_owned();
    }
    let mut word: Vec<u8> = token.bytes().collect();
    step1a(&mut word);
    step1b(&mut word);
    step1c(&mut word);
    step2(&mut word);
    step3(&mut word);
    step4(&mut word);
    step5a(&mut word);
    step5b(&mut word);
    String::from_utf8(word).expect("porter stemmer keeps its input ASCII")
}

/// Step 1a: plural suffixes.
fn step1a(word: &mut Vec<u8>) {
    if ends_with(word, b"sses") {
        replace_suffix(word, b"sses", b"ss");
    } else if ends_with(word, b"ies") {
        replace_suffix(word, b"ies", b"i");
    } else if ends_with(word, b"ss") {
        // "ss" is not a plural marker.
    } else if ends_with(word, b"s") {
        remove_suffix(word, 1);
    }
}

/// Step 1b: past tense and gerund, with the -at/-bl/-iz repair.
fn step1b(word: &mut Vec<u8>) {
    if ends_with(word, b"eed") {
        if measure(stem_before(word, b"eed")) > 0 {
            replace_suffix(word, b"eed", b"ee");
        }
        return;
    }
    let removed = if ends_with(word, b"ed") {
        2
    } else if ends_with(word, b"ing") {
        3
    } else {
        return;
    };
    let stem_len = word.len() - removed;
    if !has_vowel(&word[..stem_len]) {
        return;
    }
    word.truncate(stem_len);
    if ends_with(word, b"at") || ends_with(word, b"bl") || ends_with(word, b"iz") {
        word.push(b'e');
    } else if ends_double_consonant(word)
        && !ends_with(word, b"l")
        && !ends_with(word, b"s")
        && !ends_with(word, b"z")
    {
        remove_suffix(word, 1);
    } else if measure(word) == 1 && consonant_vowel_consonant(word) {
        word.push(b'e');
    }
}

/// Step 1c: terminal `y` after a vowel becomes `i`.
fn step1c(word: &mut Vec<u8>) {
    if ends_with(word, b"y") && has_vowel(&word[..word.len() - 1]) {
        replace_suffix(word, b"y", b"i");
    }
}

/// Step 2: derivational suffixes.
fn step2(word: &mut Vec<u8>) {
    apply_suffix_table(
        word,
        &[
            (b"ational", b"ate"),
            (b"tional", b"tion"),
            (b"enci", b"ence"),
            (b"anci", b"ance"),
            (b"izer", b"ize"),
            (b"bli", b"ble"),
            (b"alli", b"al"),
            (b"entli", b"ent"),
            (b"eli", b"e"),
            (b"ousli", b"ous"),
            (b"ization", b"ize"),
            (b"ation", b"ate"),
            (b"ator", b"ate"),
            (b"alism", b"al"),
            (b"iveness", b"ive"),
            (b"fulness", b"ful"),
            (b"ousness", b"ous"),
            (b"aliti", b"al"),
            (b"iviti", b"ive"),
            (b"biliti", b"ble"),
            (b"logi", b"log"),
        ],
    );
}

/// Step 3: further derivational suffixes.
fn step3(word: &mut Vec<u8>) {
    apply_suffix_table(
        word,
        &[
            (b"icate", b"ic"),
            (b"ative", b""),
            (b"alize", b"al"),
            (b"iciti", b"ic"),
            (b"ical", b"ic"),
            (b"ful", b""),
            (b"ness", b""),
        ],
    );
}

/// Step 4: remove suffixes that leave no derivational role.
fn step4(word: &mut Vec<u8>) {
    if ends_with(word, b"ion") {
        let stem_len = word.len() - 3;
        let preceded_by_sibilant =
            stem_len > 0 && (word[stem_len - 1] == b's' || word[stem_len - 1] == b't');
        if preceded_by_sibilant && measure(&word[..stem_len]) > 1 {
            word.truncate(stem_len);
        }
        return;
    }
    for suffix in [
        b"al".as_slice(),
        b"ance",
        b"ence",
        b"er",
        b"ic",
        b"able",
        b"ible",
        b"ant",
        b"ement",
        b"ment",
        b"ent",
        b"ou",
        b"ism",
        b"ate",
        b"iti",
        b"ous",
        b"ive",
        b"ize",
    ] {
        if ends_with(word, suffix) {
            if measure(stem_before(word, suffix)) > 1 {
                remove_suffix(word, suffix.len());
            }
            return;
        }
    }
}

/// Step 5a: a final `e` survives only where it is needed for pronunciation.
fn step5a(word: &mut Vec<u8>) {
    if !ends_with(word, b"e") {
        return;
    }
    let stem_len = word.len() - 1;
    let stem_measure = measure(&word[..stem_len]);
    if stem_measure > 1 || (stem_measure == 1 && !consonant_vowel_consonant(&word[..stem_len])) {
        word.truncate(stem_len);
    }
}

/// Step 5b: a doubled final `l` keeps only one.
fn step5b(word: &mut Vec<u8>) {
    if ends_with(word, b"ll") && measure(word) > 1 {
        remove_suffix(word, 1);
    }
}

/// Apply the first matching suffix from `table` whose removal leaves `m > 0`.
fn apply_suffix_table(word: &mut Vec<u8>, table: &[(&[u8], &[u8])]) {
    for (suffix, replacement) in table {
        if ends_with(word, suffix) {
            if measure(stem_before(word, suffix)) > 0 {
                replace_suffix(word, suffix, replacement);
            }
            return;
        }
    }
}

/// The prefix left when `suffix` is stripped; the caller checked `ends_with`.
fn stem_before<'a>(word: &'a [u8], suffix: &[u8]) -> &'a [u8] {
    &word[..word.len() - suffix.len()]
}

/// `true` when `word` carries `suffix`.
fn ends_with(word: &[u8], suffix: &[u8]) -> bool {
    word.ends_with(suffix)
}

/// Drop the last `n` bytes.
fn remove_suffix(word: &mut Vec<u8>, n: usize) {
    word.truncate(word.len() - n);
}

/// Drop `suffix` and append `replacement`.
fn replace_suffix(word: &mut Vec<u8>, suffix: &[u8], replacement: &[u8]) {
    remove_suffix(word, suffix.len());
    word.extend_from_slice(replacement);
}

/// The Porter consonant test, including the `y` rule.
fn is_consonant(word: &[u8], i: usize) -> bool {
    match word[i] {
        b'a' | b'e' | b'i' | b'o' | b'u' => false,
        // `y` counts as a consonant when the letter before it is a vowel;
        // after a consonant it is the vowel that carries the syllable.
        b'y' => i == 0 || !is_consonant(word, i - 1),
        _ => true,
    }
}

/// `m`: the number of vowel-consonant sequences in `word`.
fn measure(word: &[u8]) -> usize {
    let len = word.len();
    let mut i = 0;
    while i < len && is_consonant(word, i) {
        i += 1;
    }
    let mut n = 0;
    while i < len {
        while i < len && !is_consonant(word, i) {
            i += 1;
        }
        if i >= len {
            break;
        }
        n += 1;
        while i < len && is_consonant(word, i) {
            i += 1;
        }
    }
    n
}

/// `*v*`: `word` contains a vowel.
fn has_vowel(word: &[u8]) -> bool {
    (0..word.len()).any(|i| !is_consonant(word, i))
}

/// `*d`: `word` ends in a doubled consonant.
fn ends_double_consonant(word: &[u8]) -> bool {
    let len = word.len();
    len >= 2 && word[len - 1] == word[len - 2] && is_consonant(word, len - 1)
}

/// `*o`: `word` ends consonant-vowel-consonant, last consonant not w/x/y.
fn consonant_vowel_consonant(word: &[u8]) -> bool {
    let len = word.len();
    if len < 3 {
        return false;
    }
    let (c1, v, c2) = (len - 3, len - 2, len - 1);
    is_consonant(word, c1)
        && !is_consonant(word, v)
        && is_consonant(word, c2)
        && !matches!(word[c2], b'w' | b'x' | b'y')
}

#[cfg(test)]
#[path = "lexical_porter_tests.rs"]
mod tests;
