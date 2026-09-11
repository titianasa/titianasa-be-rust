// Phase 37 — port of parelabs' `lib/quiz/score.ts::scoreSingle`, scoring
// one `QuizQuestion` against the learner's answer map.
//
// The dispatch is by comparison FAMILY, not by subtype: ~40 subtypes
// collapse to five behaviours (set-answer, needs-eval, reorder,
// fuzzy-text, exact-key), which is why adding a subtype rarely needs a
// new arm here at all — it just joins one of the sets below.
//
// This is separate from `grading.rs`, which grades the legacy
// `questions`-table shape (typed envelopes like `{"index": 2}`). A
// quiz_config question's key is a plain string or array, so the two
// contracts stay apart rather than growing a shared, ambiguous one.

use std::collections::HashMap;

use serde_json::Value;

use crate::services::answer_match::{matches_answer, matches_answer_with_options, matches_reorder};
use crate::services::quiz_config::{QuizQuestion, QuizQuestionGroup};

/// Compared with `matches_answer_with_options` — surface-variation
/// tolerant, and aware of a word-bank pool when the group has one.
const FUZZY_TEXT: &[&str] = &[
    "gap_fill",
    "table_completion",
    "flow_chart",
    "matching",
    "matching_headings",
    "map_labeling",
    "short_answer",
    "vocab_cloze",
    "spelling",
    "word_match",
    "cloze_passage",
    "error_correction",
    "word_form",
    "paragraph_editing",
];

/// Sequence answers — order matters, punctuation doesn't.
const REORDER: &[&str] = &["sentence_reorder", "word_scramble"];

/// Answer is a set of keys. Scored per-slot with partial credit and no
/// negative marking, matching official IELTS "choose TWO" behaviour.
const SET_ANSWER: &[&str] = &[
    "multiple_choice_multiple",
    // Indices rather than letters, but the same sorted-set comparison.
    "highlight_incorrect_words",
];

/// No deterministic verdict — an LLM rubric or a human decides later, so
/// these score 0 with `needs_eval` set rather than 0 for being wrong.
const NEEDS_EVAL: &[&str] = &[
    "essay",
    "voice_record",
    "video_record",
    "file_upload",
    "image_answer",
    // Owns its internal scoring; completion is tracked, the score isn't.
    "h5p",
    "widget_interact",
    // Freeform paraphrase / pronunciation — not fixed-key gradable.
    "sentence_transform",
    "listen_repeat",
    "intonation",
    // No right answer by design.
    "likert_scale",
    "flashcard",
    // Scored in person by the tutor against a rubric.
    "speaking_challenge",
];

#[derive(Debug, Clone, PartialEq)]
pub struct QuestionScore {
    pub is_correct: bool,
    pub points_earned: i64,
    pub points_max: i64,
    pub needs_eval: bool,
}

/// The learner's submitted answers, keyed by `QuizQuestion::key()`.
pub type AnswerState = HashMap<String, Value>;

fn as_string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn as_str(value: Option<&Value>) -> &str {
    value.and_then(|v| v.as_str()).unwrap_or("")
}

fn weight_of(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

/// The learner's stored answer is the chip's display form ("A. Menolak
/// dengan sopan"), while `option_scores` is keyed by the bare label
/// ("A") — match the two without needing the option pool here.
fn matches_option_label(picked: &str, label: &str) -> bool {
    let trimmed = picked.trim();
    trimmed
        .split_once(['.', ')', ':'])
        .map(|(prefix, _)| prefix.trim().eq_ignore_ascii_case(label.trim()))
        .unwrap_or(false)
}

pub fn score_question(subtype: &str, question: &QuizQuestion, answers: &AnswerState, group: Option<&QuizQuestionGroup>) -> QuestionScore {
    let key = question.key();
    let submitted = answers.get(&key);

    // CPNS TKP and BUMN AKHLAK have no single right answer: every option
    // carries a weight (typically 1-5) and the score is whatever the
    // learner picked. Detected by the presence of `option_scores` rather
    // than by a separate subtype, because the question is otherwise an
    // ordinary multiple choice and is authored, generated and rendered
    // as one.
    if let Some(weights) = &question.option_scores {
        let max = weights.values().filter_map(weight_of).fold(0.0_f64, f64::max);
        let picked = as_str(submitted);
        let earned = weights
            .iter()
            .find(|(label, _)| label.as_str() == picked || matches_option_label(picked, label))
            .and_then(|(_, v)| weight_of(v))
            .unwrap_or(0.0);
        return QuestionScore {
            // "Correct" means the best-scoring option, so a dashboard
            // still has something meaningful to show.
            is_correct: max > 0.0 && (earned - max).abs() < f64::EPSILON,
            points_earned: earned.round() as i64,
            points_max: max.round() as i64,
            needs_eval: false,
        };
    }

    if SET_ANSWER.contains(&subtype) {
        // A multi-mark question covers several slots ("5-6" = 2 points);
        // each correctly-picked member earns one, with no penalty for a
        // wrong pick beyond not earning its point.
        let slots = question.slot_count();
        let mut user: Vec<String> = as_string_list(submitted);
        let mut correct: Vec<String> = as_string_list(question.answer.as_ref());
        let earned = user.iter().filter(|choice| correct.contains(choice)).count() as i64;
        user.sort();
        correct.sort();
        return QuestionScore {
            is_correct: !correct.is_empty() && user == correct,
            points_earned: earned.min(slots),
            points_max: slots,
            needs_eval: false,
        };
    }

    if NEEDS_EVAL.contains(&subtype) {
        return QuestionScore { is_correct: false, points_earned: 0, points_max: 1, needs_eval: true };
    }

    if REORDER.contains(&subtype) {
        let ok = matches_reorder(as_str(submitted), question.answer.as_ref());
        return QuestionScore { is_correct: ok, points_earned: i64::from(ok), points_max: 1, needs_eval: false };
    }

    if FUZZY_TEXT.contains(&subtype) {
        // Word-bank subtypes store the option's full display form
        // ("D. subjective assessment") as the answer, while the key is
        // usually authored bare — resolving through the pool is what
        // makes a correct pick actually score.
        let empty = Vec::new();
        let pool = match group {
            Some(g) if subtype == "matching_headings" => &g.headings,
            Some(g) => &g.options,
            None => &empty,
        };
        let ok = matches_answer_with_options(as_str(submitted), question.answer.as_ref(), pool);
        return QuestionScore { is_correct: ok, points_earned: i64::from(ok), points_max: 1, needs_eval: false };
    }

    // Exact-key subtypes: the answer is a fixed token (a choice letter,
    // "True"/"False"/"Not Given", ...) where fuzziness would only ever
    // let a wrong answer through.
    if is_exact_key_match(subtype) {
        let user = as_str(submitted);
        let correct = question.answer.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        let ok = !user.is_empty() && user == correct;
        return QuestionScore { is_correct: ok, points_earned: i64::from(ok), points_max: 1, needs_eval: false };
    }

    // Unknown subtype — fall back to fuzzy, which is the safer default
    // for something we have no specific contract for.
    let ok = matches_answer(as_str(submitted), question.answer.as_ref());
    QuestionScore { is_correct: ok, points_earned: i64::from(ok), points_max: 1, needs_eval: false }
}

/// Subtypes whose answer is a fixed token compared verbatim.
fn is_exact_key_match(subtype: &str) -> bool {
    matches!(
        subtype,
        "multiple_choice"
            | "true_false"
            | "true_false_not_given"
            | "yes_no_not_given"
            | "error_identification"
            | "image_word"
            | "stress_pattern"
            | "minimal_pairs"
            | "analogy"
    )
}

/// Whether a subtype defers its verdict to an LLM rubric or a human.
pub fn needs_evaluation(subtype: &str) -> bool {
    NEEDS_EVAL.contains(&subtype)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn question(value: serde_json::Value) -> QuizQuestion {
        serde_json::from_value(value).unwrap()
    }

    fn answers(pairs: &[(&str, serde_json::Value)]) -> AnswerState {
        pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
    }

    #[test]
    fn exact_key_subtype_requires_the_exact_token() {
        let q = question(json!({"number": 1, "answer": "B"}));
        let score = score_question("multiple_choice", &q, &answers(&[("1", json!("B"))]), None);
        assert!(score.is_correct);
        assert_eq!(score.points_earned, 1);

        let wrong = score_question("multiple_choice", &q, &answers(&[("1", json!("C"))]), None);
        assert!(!wrong.is_correct);
    }

    #[test]
    fn an_unanswered_question_scores_zero_rather_than_erroring() {
        let q = question(json!({"number": 1, "answer": "B"}));
        let score = score_question("multiple_choice", &q, &answers(&[]), None);
        assert!(!score.is_correct);
        assert_eq!(score.points_earned, 0);
        assert_eq!(score.points_max, 1);
    }

    #[test]
    fn fuzzy_text_subtype_forgives_surface_variation() {
        let q = question(json!({"number": 3, "answer": "10 years"}));
        let score = score_question("short_answer", &q, &answers(&[("3", json!("  Ten Years. "))]), None);
        assert!(score.is_correct);
    }

    #[test]
    fn multi_mark_range_gives_partial_credit_per_slot() {
        // IELTS "choose TWO": one right of two picks earns 1 of 2 points
        // but is not marked fully correct.
        let q = question(json!({"number": "5-6", "answer": ["A", "C"]}));
        let partial = score_question("multiple_choice_multiple", &q, &answers(&[("5-6", json!(["A", "D"]))]), None);
        assert!(!partial.is_correct);
        assert_eq!(partial.points_earned, 1);
        assert_eq!(partial.points_max, 2);

        let full = score_question("multiple_choice_multiple", &q, &answers(&[("5-6", json!(["C", "A"]))]), None);
        assert!(full.is_correct);
        assert_eq!(full.points_earned, 2);
    }

    #[test]
    fn a_wrong_extra_pick_is_not_negatively_marked() {
        let q = question(json!({"number": "5-6", "answer": ["A", "C"]}));
        let score = score_question("multiple_choice_multiple", &q, &answers(&[("5-6", json!(["A", "C", "E"]))]), None);
        assert!(!score.is_correct);
        assert_eq!(score.points_earned, 2, "the two right picks still count");
    }

    #[test]
    fn production_subtypes_defer_instead_of_scoring_zero_as_wrong() {
        let q = question(json!({"number": 1, "prompt": "Describe your weekend."}));
        let score = score_question("essay", &q, &answers(&[("1", json!("I went hiking..."))]), None);
        assert!(score.needs_eval);
        assert_eq!(score.points_earned, 0);
        assert!(!score.is_correct);
    }

    #[test]
    fn reorder_requires_the_right_order() {
        let q = question(json!({"number": 1, "answer": "She goes to school every day"}));
        assert!(score_question("sentence_reorder", &q, &answers(&[("1", json!("she goes to school every day"))]), None).is_correct);
        assert!(!score_question("sentence_reorder", &q, &answers(&[("1", json!("every day she goes to school"))]), None).is_correct);
    }

    #[test]
    fn word_bank_answer_resolves_through_the_groups_option_pool() {
        let group: QuizQuestionGroup = serde_json::from_value(json!({
            "group_id": "g1",
            "type": "matching",
            "options": [{"label": "D", "text": "subjective assessment"}],
            "questions": [],
        }))
        .unwrap();
        let q = question(json!({"number": 2, "answer": "subjective assessment"}));
        let score = score_question("matching", &q, &answers(&[("2", json!("D. subjective assessment"))]), Some(&group));
        assert!(score.is_correct, "the learner picked the right chip; the key is authored bare");
    }

    #[test]
    fn matching_headings_reads_the_headings_pool_not_options() {
        let group: QuizQuestionGroup = serde_json::from_value(json!({
            "group_id": "g1",
            "type": "matching_headings",
            "headings": [{"label": "iv", "text": "A surprising discovery"}],
            "questions": [],
        }))
        .unwrap();
        let q = question(json!({"number": 1, "answer": "A surprising discovery"}));
        assert!(score_question("matching_headings", &q, &answers(&[("1", json!("iv. A surprising discovery"))]), Some(&group)).is_correct);
    }

    #[test]
    fn an_unknown_subtype_falls_back_to_fuzzy_rather_than_panicking() {
        let q = question(json!({"number": 1, "answer": "blue"}));
        let score = score_question("some_future_subtype", &q, &answers(&[("1", json!("Blue"))]), None);
        assert!(score.is_correct);
    }
}
