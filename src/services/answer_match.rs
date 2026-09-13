// Phase 37 — port of parelabs' `lib/answer-match.ts`. Pure functions,
// no DB access.
//
// The principle it encodes: give the learner the benefit of the doubt on
// surface variation (case, punctuation, articles, hyphens, numeral vs
// number-word, % vs "percent") while still requiring the right content
// word. These are the Cambridge/IELTS marking conventions, and Titian
// grades against the same expectations now that a deck can be authored
// in that format.
//
// This lives beside `grading.rs` rather than inside it because the old
// grading path compares typed answer envelopes (`{index: 2}`), while a
// quiz_config question's key is a plain string ("blue|azure") — two
// different contracts that would tangle if merged.

use std::collections::HashSet;

use crate::services::quiz_config::{value_to_key, LabeledOption};

/// A quantitative "isian singkat" answer very often comes back as a
/// JSON *number*, not a string — `Value::as_str()` returns `None` for
/// that, so any caller using it directly silently treated a numeric
/// answer as absent. This is the one conversion every scalar extraction
/// here goes through instead.
fn value_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(_) => Some(value_to_key(v)),
        _ => None,
    }
}

/// Bidirectional numeral/number-word aliases. Only kick in when the text
/// actually contains one side of a pair, so "one-off" doesn't become
/// "1-off" in unrelated answers.
const NUMBER_WORD_PAIRS: &[(&str, &str)] = &[
    ("0", "zero"),
    ("1", "one"),
    ("2", "two"),
    ("3", "three"),
    ("4", "four"),
    ("5", "five"),
    ("6", "six"),
    ("7", "seven"),
    ("8", "eight"),
    ("9", "nine"),
    ("10", "ten"),
    ("11", "eleven"),
    ("12", "twelve"),
    ("13", "thirteen"),
    ("14", "fourteen"),
    ("15", "fifteen"),
    ("16", "sixteen"),
    ("17", "seventeen"),
    ("18", "eighteen"),
    ("19", "nineteen"),
    ("20", "twenty"),
    ("30", "thirty"),
    ("40", "forty"),
    ("50", "fifty"),
    ("60", "sixty"),
    ("70", "seventy"),
    ("80", "eighty"),
    ("90", "ninety"),
    ("100", "hundred"),
    ("1000", "thousand"),
];

const TRIM_PUNCTUATION: &[char] = &['.', ',', ';', ':', '!', '?', '\'', '"', '(', ')', '[', ']', '{', '}', '-', ' ', '\t', '\n'];

/// Normalise text before comparison: unify quote/dash glyphs, expand `%`,
/// collapse whitespace, strip surrounding punctuation, lowercase.
pub fn normalize_answer(s: &str) -> String {
    let replaced: String = s
        .chars()
        .map(|c| match c {
            '\u{00a0}' => ' ',
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201c}' | '\u{201d}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            other => other,
        })
        .collect();
    let percent_expanded = replaced.replace('%', " percent");
    let collapsed = percent_expanded.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_matches(|c: char| TRIM_PUNCTUATION.contains(&c)).to_lowercase().trim().to_string()
}

fn word_boundary_replace(haystack: &str, from: &str, to: &str) -> Option<String> {
    // A hand-rolled `\bfrom\b` — the regex crate isn't a dependency here
    // and this only ever runs over short answer strings.
    let mut out = String::with_capacity(haystack.len());
    let bytes = haystack.as_bytes();
    let from_bytes = from.as_bytes();
    let mut i = 0;
    let mut replaced_any = false;
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';

    while i < bytes.len() {
        let matches_here = i + from_bytes.len() <= bytes.len()
            && &bytes[i..i + from_bytes.len()] == from_bytes
            && (i == 0 || !is_word(bytes[i - 1]))
            && (i + from_bytes.len() == bytes.len() || !is_word(bytes[i + from_bytes.len()]));
        if matches_here {
            out.push_str(to);
            i += from_bytes.len();
            replaced_any = true;
        } else {
            // Push one whole UTF-8 char at a time so multi-byte text is
            // never split mid-codepoint.
            let ch = haystack[i..].chars().next().expect("index is on a char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    replaced_any.then_some(out)
}

fn number_aliases(s: &str) -> Vec<String> {
    let mut out = vec![s.to_string()];
    for (num, word) in NUMBER_WORD_PAIRS {
        if let Some(swapped) = word_boundary_replace(s, num, word) {
            out.push(swapped);
        }
        if let Some(swapped) = word_boundary_replace(s, word, num) {
            out.push(swapped);
        }
    }
    out
}

/// IELTS marking treats a leading "a/an/the" as optional.
fn strip_leading_article(s: &str) -> String {
    for article in ["the ", "an ", "a "] {
        if let Some(rest) = s.strip_prefix(article) {
            return rest.trim().to_string();
        }
    }
    s.to_string()
}

/// Every surface form of one normalised string we're willing to accept.
fn expand_variants(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let with_hyphen = s.split_whitespace().collect::<Vec<_>>().join("-");
    let with_space = s.replace('-', " ");
    let no_article = strip_leading_article(s);
    let no_article_hyphen = {
        let stripped = ["the-", "an-", "a-"].iter().find_map(|p| with_hyphen.strip_prefix(*p)).unwrap_or(&with_hyphen);
        stripped.trim_start_matches('-').to_string()
    };
    let no_article_space = strip_leading_article(&with_space);

    for base in [s.to_string(), with_hyphen, with_space, no_article, no_article_hyphen, no_article_space] {
        if base.is_empty() {
            continue;
        }
        for variant in number_aliases(&base) {
            out.insert(variant);
        }
    }
    out
}

/// Expand an answer key into every valid representation. A key may be a
/// bare string, a string carrying `/`, `;` or `|` separated alternates,
/// or an array of either.
pub fn expand_answer_key(correct: Option<&serde_json::Value>) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(correct) = correct else { return out };

    let raw_items: Vec<String> = match correct {
        serde_json::Value::Array(items) => items.iter().filter_map(value_text).collect(),
        other => match value_text(other) {
            Some(s) => vec![s],
            None => return out,
        },
    };

    for raw in raw_items {
        for alt in raw.split(['/', ';', '|']) {
            let normalised = normalize_answer(alt);
            if normalised.is_empty() {
                continue;
            }
            out.extend(expand_variants(&normalised));
        }
    }
    out
}

/// Parse a number the way an Indonesian learner writes one: "1.234,5"
/// (dot thousands, comma decimal) and "1,234.5" (the English convention)
/// both mean the same value, and either may be typed in the same exam.
/// Returns None for anything that isn't purely numeric.
fn parse_number(s: &str) -> Option<f64> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() || !cleaned.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+')) {
        return None;
    }
    let last_dot = cleaned.rfind('.');
    let last_comma = cleaned.rfind(',');
    let normalised = match (last_dot, last_comma) {
        // Whichever separator comes LAST is the decimal point; the other
        // is grouping.
        (Some(d), Some(c)) if c > d => cleaned.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => cleaned.replace(',', ""),
        // A lone separator is a decimal point unless it groups digits in
        // threes ("1.234" is one thousand two hundred, "1.5" is one and a
        // half) — the ambiguity real exams live with.
        (None, Some(c)) => {
            if cleaned.len() - c - 1 == 3 {
                cleaned.replace(',', "")
            } else {
                cleaned.replace(',', ".")
            }
        }
        (Some(d), None) => {
            if cleaned.len() - d - 1 == 3 {
                cleaned.replace('.', "")
            } else {
                cleaned.clone()
            }
        }
        (None, None) => cleaned.clone(),
    };
    normalised.parse::<f64>().ok()
}

/// Loose text equality for gap fill / short answer / matching labels.
/// Empty on either side is always a miss.
pub fn matches_answer(user: &str, correct: Option<&serde_json::Value>) -> bool {
    let normalised_user = normalize_answer(user);
    if normalised_user.is_empty() {
        return false;
    }
    let keys = expand_answer_key(correct);
    if keys.is_empty() {
        return false;
    }
    if expand_variants(&normalised_user).iter().any(|variant| keys.contains(variant)) {
        return true;
    }

    // Numeric equality, so a SNBT "isian singkat" answer of 1,5 matches a
    // key of 1.5 — and 1.000 matches 1000. Only applies when BOTH sides
    // are numbers, so ordinary words are unaffected.
    if let Some(user_number) = parse_number(&normalised_user) {
        return keys.iter().filter_map(|k| parse_number(k)).any(|key_number| (key_number - user_number).abs() < 1e-9);
    }
    false
}

/// Sentence-arrangement comparison. Stricter than `matches_answer`
/// because word order is the thing being tested, but still forgiving of
/// punctuation, case and spacing.
pub fn matches_reorder(user: &str, correct: Option<&serde_json::Value>) -> bool {
    fn strip(s: &str) -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    let stripped_user = strip(user);
    if stripped_user.is_empty() {
        return false;
    }

    let raw_keys: Vec<String> = match correct {
        Some(serde_json::Value::Array(items)) => items.iter().filter_map(value_text).collect(),
        Some(other) => match value_text(other) {
            Some(s) => vec![s],
            None => return false,
        },
        None => return false,
    };

    raw_keys.iter().flat_map(|k| k.split('|')).any(|k| strip(k) == stripped_user)
}

/// The chip's display form — how a word-bank pick is stored and shown.
fn chip_label(option: &LabeledOption) -> String {
    match option.label() {
        Some(label) => format!("{label}. {}", option.text()),
        None => option.text().to_string(),
    }
}

/// Split a leading "A. " / "A) " / "A: " prefix off a plain-string option
/// that embeds its own label.
fn strip_inline_label_prefix(s: &str) -> Option<String> {
    let mut chars = s.char_indices();
    let mut label_len = 0;
    for _ in 0..2 {
        match chars.next() {
            Some((idx, c)) if c.is_ascii_alphanumeric() => label_len = idx + c.len_utf8(),
            _ => break,
        }
    }
    if label_len == 0 {
        return None;
    }
    let rest = &s[label_len..];
    let rest = rest.strip_prefix(['.', ')', ':'])?;
    let trimmed = rest.trim_start();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Looser cousin: also accepts a 4-char label (roman numerals) and a bare
/// space as the separator, because content authors key matching answers
/// in whatever style they typed the pool in ("V. Goals", "V Goals",
/// "v Goals" have all been seen for the same heading).
fn strip_loose_label_prefix(s: &str) -> Option<String> {
    let mut label_end = 0;
    for (idx, c) in s.char_indices().take(4) {
        if c.is_ascii_alphanumeric() {
            label_end = idx + c.len_utf8();
        } else {
            break;
        }
    }
    if label_end == 0 || label_end >= s.len() {
        return None;
    }
    let rest = &s[label_end..];
    let rest = match rest.strip_prefix(['.', ')', ':']) {
        Some(r) => r.trim_start(),
        None if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        None => return None,
    };
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Resolve a stored word-bank value ("D. subjective assessment") to the
/// option's bare text, for comparison only — never changes what's stored
/// or displayed. Falls through unchanged when nothing matches.
fn resolve_option_answer_value(raw: &str, options: &[LabeledOption]) -> String {
    if raw.is_empty() || options.is_empty() {
        return raw.to_string();
    }
    for option in options {
        match option {
            LabeledOption::Text(s) if s == raw => {
                return strip_inline_label_prefix(s).unwrap_or_else(|| s.clone());
            }
            LabeledOption::Text(_) => {}
            LabeledOption::Labeled { text, .. } => {
                if chip_label(option) == raw || text == raw {
                    return text.clone();
                }
            }
        }
    }
    raw.to_string()
}

/// Same lookup, resolving to the bare LABEL ("H") instead of the text —
/// matching-type keys are authored either way.
fn resolve_option_label_value(raw: &str, options: &[LabeledOption]) -> String {
    if raw.is_empty() || options.is_empty() {
        return raw.to_string();
    }
    for option in options {
        if let LabeledOption::Labeled { label: Some(label), text, .. } = option {
            if chip_label(option) == raw || text == raw {
                return label.clone();
            }
        }
    }
    raw.to_string()
}

/// Word-bank-aware `matches_answer`. Tries, in order: the raw stored
/// value (some keys are authored WITH the label prefix), the
/// prefix-resolved bare text, the bare label letter, and finally the key
/// with its own loose label prefix stripped. Purely additive over
/// `matches_answer` — it can turn a wrong answer right, never the
/// reverse — so it is safe wherever a word-bank value is compared.
pub fn matches_answer_with_options(user: &str, correct: Option<&serde_json::Value>, options: &[LabeledOption]) -> bool {
    let bare_text = resolve_option_answer_value(user, options);
    if matches_answer(user, correct)
        || matches_answer(&bare_text, correct)
        || matches_answer(&resolve_option_label_value(user, options), correct)
    {
        return true;
    }

    let key_candidates: Vec<String> = match correct {
        Some(serde_json::Value::Array(items)) => items.iter().filter_map(value_text).collect(),
        Some(other) => value_text(other).into_iter().collect(),
        None => Vec::new(),
    };
    key_candidates.iter().any(|k| match strip_loose_label_prefix(k) {
        Some(rest) => matches_answer(&bare_text, Some(&serde_json::Value::String(rest))),
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(v: serde_json::Value) -> serde_json::Value {
        v
    }

    #[test]
    fn ignores_case_whitespace_and_surrounding_punctuation() {
        let k = key(json!("blue"));
        assert!(matches_answer("Blue", Some(&k)));
        assert!(matches_answer("  blue.  ", Some(&k)));
        assert!(!matches_answer("green", Some(&k)));
    }

    #[test]
    fn an_empty_answer_never_matches() {
        let k = key(json!("blue"));
        assert!(!matches_answer("", Some(&k)));
        assert!(!matches_answer("   ", Some(&k)));
        // ...and neither does a present answer against an absent key.
        assert!(!matches_answer("blue", None));
    }

    #[test]
    fn accepts_pipe_slash_and_semicolon_alternates() {
        let k = key(json!("blue|azure"));
        assert!(matches_answer("azure", Some(&k)));
        let k2 = key(json!("lift/elevator"));
        assert!(matches_answer("elevator", Some(&k2)));
        let k3 = key(json!(["ten", "10"]));
        assert!(matches_answer("10", Some(&k3)));
    }

    #[test]
    fn treats_numerals_and_number_words_as_equivalent() {
        let k = key(json!("10 years"));
        assert!(matches_answer("ten years", Some(&k)));
        let k2 = key(json!("ten years"));
        assert!(matches_answer("10 years", Some(&k2)));
    }

    #[test]
    fn leading_articles_are_optional_and_hyphens_flex() {
        let k = key(json!("the open-plan office"));
        assert!(matches_answer("open plan office", Some(&k)));
        assert!(matches_answer("an open-plan office", Some(&k)));
    }

    #[test]
    fn percent_symbol_and_word_are_interchangeable() {
        let k = key(json!("50%"));
        assert!(matches_answer("50 percent", Some(&k)));
    }

    #[test]
    fn reorder_requires_the_right_word_order() {
        let k = key(json!("She goes to school every day."));
        assert!(matches_reorder("she goes to school every day", Some(&k)));
        assert!(!matches_reorder("school every day she goes to", Some(&k)));
    }

    #[test]
    fn word_bank_value_matches_a_bare_text_key() {
        // The real bug this guards: the learner's stored answer is the
        // chip's display form, but the key is authored as bare text.
        let options = vec![LabeledOption::Labeled { label: Some("D".to_string()), text: "subjective assessment".to_string(), image: None }];
        let k = key(json!("subjective assessment"));
        assert!(matches_answer_with_options("D. subjective assessment", Some(&k), &options));
    }

    #[test]
    fn word_bank_value_matches_a_bare_label_key() {
        let options = vec![LabeledOption::Labeled { label: Some("H".to_string()), text: "strategic alliance".to_string(), image: None }];
        let k = key(json!("H"));
        assert!(matches_answer_with_options("H. strategic alliance", Some(&k), &options));
    }

    #[test]
    fn word_bank_value_matches_a_key_carrying_its_own_loose_prefix() {
        let options = vec![LabeledOption::Labeled { label: Some("v".to_string()), text: "Two clear educational goals".to_string(), image: None }];
        let k = key(json!("v Two clear educational goals"));
        assert!(matches_answer_with_options("v. Two clear educational goals", Some(&k), &options));
    }

    #[test]
    fn plain_string_option_embedding_its_own_label_still_resolves() {
        let options = vec![LabeledOption::Text("A. canopy layer".to_string())];
        let k = key(json!("canopy layer"));
        assert!(matches_answer_with_options("A. canopy layer", Some(&k), &options));
    }

    #[test]
    fn with_options_never_turns_a_right_answer_wrong() {
        // It is only ever additive over matches_answer.
        let options = vec![LabeledOption::Labeled { label: Some("A".to_string()), text: "one".to_string(), image: None }];
        let k = key(json!("blue"));
        assert!(matches_answer_with_options("blue", Some(&k), &options));
        assert!(!matches_answer_with_options("red", Some(&k), &options));
    }

    #[test]
    fn numbers_match_across_indonesian_and_english_decimal_conventions() {
        // The same value, typed the way each convention writes it.
        let k = key(json!("1.5"));
        assert!(matches_answer("1,5", Some(&k)));
        let k2 = key(json!("1,5"));
        assert!(matches_answer("1.5", Some(&k2)));
        // Thousands separators are grouping, not decimals.
        let k3 = key(json!("1000"));
        assert!(matches_answer("1.000", Some(&k3)));
        assert!(matches_answer("1,000", Some(&k3)));
        // Still wrong when the value differs.
        assert!(!matches_answer("1,6", Some(&k)));
    }

    // Regression — the QA sweep found "isian singkat" numeric answers
    // failing near-universally: the model authors the key as a bare
    // JSON number, and `expand_answer_key`'s array/string-only match
    // silently discarded it (returning an empty key set) before any
    // comparison ran.
    #[test]
    fn a_bare_json_number_key_is_not_silently_discarded() {
        let k = key(json!(42));
        assert!(matches_answer("42", Some(&k)));
        let k2 = key(json!(1.5));
        assert!(matches_answer("1,5", Some(&k2)));
    }

    #[test]
    fn numeric_leniency_does_not_touch_ordinary_words() {
        let k = key(json!("dua"));
        assert!(!matches_answer("3", Some(&k)));
        // ...but the existing number-word aliasing still applies.
        let k2 = key(json!("2 jam"));
        assert!(matches_answer("two jam", Some(&k2)));
    }

    #[test]
    fn multibyte_text_is_never_split_mid_character() {
        // word_boundary_replace walks bytes; a naive index bump would
        // panic or corrupt on non-ASCII content (Titian grades Indonesian,
        // Arabic and CJK answers too).
        let k = key(json!("kucing"));
        assert!(matches_answer("kucing", Some(&k)));
        let arabic = key(json!("قطة"));
        assert!(matches_answer("قطة", Some(&arabic)));
        let cjk = key(json!("猫 3 匹"));
        assert!(matches_answer("猫 three 匹", Some(&cjk)));
    }
}
