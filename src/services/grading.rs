// Port of grading.ts. Pure functions, no DB access.
//
// Phase 37 — `is_auto_gradable` now mirrors quiz_subtype.rs's registry
// (every `GradingMode::Auto` entry), not just the original 6 types.
// `mcq`/`fill_blank` renamed to `multiple_choice`/`gap_fill` (see
// migrations/0038); `is_correct` below groups the ~28 auto subtypes
// into the same handful of shared comparators question_schema.rs's
// validators use, rather than one bespoke arm each.

pub fn is_auto_gradable(question_type: &str) -> bool {
    matches!(
        question_type,
        "multiple_choice"
            | "gap_fill"
            | "matching"
            | "true_false_not_given"
            | "matching_headings"
            | "short_answer"
            | "multiple_choice_multiple"
            | "true_false"
            | "yes_no_not_given"
            | "minimal_pairs"
            | "stress_pattern"
            | "intonation"
            | "word_form"
            | "sentence_transform"
            | "vocab_cloze"
            | "spelling"
            | "image_word"
            | "analogy"
            | "cloze_passage"
            | "error_correction"
            | "word_match"
            | "table_completion"
            | "flow_chart"
            | "map_labeling"
            | "sentence_reorder"
            | "word_scramble"
            | "highlight_incorrect_words"
            | "error_identification"
    )
}

fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

fn as_object(value: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    value.as_object()
}

// Exact/normalized match. A malformed or absent submitted answer just
// grades as incorrect rather than erroring the whole submit — the "did
// they answer at all" check happens earlier, against the raw answers
// map, not here. `data` is the question's own data column, only
// short_answer needs it (max_words word-limit gate).
pub fn is_correct(question_type: &str, correct_answer: &serde_json::Value, submitted: &serde_json::Value, data: Option<&serde_json::Value>) -> bool {
    let correct_obj = as_object(correct_answer);
    let submitted_obj = as_object(submitted);

    match question_type {
        "multiple_choice" | "minimal_pairs" | "stress_pattern" | "intonation" => {
            let correct = correct_obj.and_then(|o| o.get("index")).and_then(|v| v.as_i64());
            let submitted_index = submitted_obj.and_then(|o| o.get("index")).and_then(|v| v.as_i64());
            correct.is_some() && correct == submitted_index
        }
        "gap_fill" | "word_form" | "sentence_transform" | "vocab_cloze" | "spelling" | "image_word" | "analogy" | "cloze_passage" | "error_correction" => {
            let correct = correct_obj.and_then(|o| o.get("text")).and_then(|v| v.as_str()).map(normalize);
            let submitted_text = submitted_obj.and_then(|o| o.get("text")).and_then(|v| v.as_str()).map(normalize);
            correct.is_some() && correct == submitted_text
        }
        // Order-independent set of indices — multiple_choice_multiple's
        // answer is "which subset", not "which one".
        "multiple_choice_multiple" => {
            fn index_set(v: Option<&serde_json::Value>) -> Option<Vec<i64>> {
                let mut xs: Vec<i64> = v?.as_array()?.iter().map(|x| x.as_i64()).collect::<Option<_>>()?;
                xs.sort_unstable();
                Some(xs)
            }
            let correct = index_set(correct_obj.and_then(|o| o.get("indices")));
            let submitted = index_set(submitted_obj.and_then(|o| o.get("indices")));
            match (correct, submitted) {
                (Some(c), Some(s)) => !c.is_empty() && c == s,
                _ => false,
            }
        }
        // Same shape/rule as multiple_choice_multiple — a target SET
        // of token indices (which tokens are the incorrect/flagged
        // ones), no partial credit.
        "highlight_incorrect_words" | "error_identification" => {
            fn index_set(v: Option<&serde_json::Value>) -> Option<Vec<i64>> {
                let mut xs: Vec<i64> = v?.as_array()?.iter().map(|x| x.as_i64()).collect::<Option<_>>()?;
                xs.sort_unstable();
                Some(xs)
            }
            let correct = index_set(correct_obj.and_then(|o| o.get("indices")));
            let submitted = index_set(submitted_obj.and_then(|o| o.get("indices")));
            match (correct, submitted) {
                (Some(c), Some(s)) => !c.is_empty() && c == s,
                _ => false,
            }
        }
        "true_false" | "yes_no_not_given" => {
            let correct = correct_obj.and_then(|o| o.get("value")).and_then(|v| v.as_str());
            let submitted_value = submitted_obj.and_then(|o| o.get("value")).and_then(|v| v.as_str());
            correct.is_some() && correct == submitted_value
        }
        // Exact sequence match — a reorder/scramble answer is only
        // correct if every position matches, same "no partial credit"
        // rule as everything else here.
        "sentence_reorder" | "word_scramble" => {
            let correct = correct_obj.and_then(|o| o.get("order")).and_then(|v| v.as_array());
            let submitted = submitted_obj.and_then(|o| o.get("order")).and_then(|v| v.as_array());
            match (correct, submitted) {
                (Some(c), Some(s)) => !c.is_empty() && c == s,
                _ => false,
            }
        }
        // Generalized matching_headings shape (`assignments` map) for
        // the newer assignment-style siblings — same "every key must
        // match" rule as matching_headings below.
        "table_completion" | "flow_chart" | "map_labeling" => {
            let correct_assignments = correct_obj.and_then(|o| o.get("assignments")).and_then(as_object);
            let submitted_assignments = submitted_obj.and_then(|o| o.get("assignments")).and_then(as_object);
            match (correct_assignments, submitted_assignments) {
                (Some(correct), Some(submitted)) => !correct.is_empty() && correct.iter().all(|(id, v)| submitted.get(id) == Some(v)),
                _ => false,
            }
        }
        // Order-independent set comparison — the pairing itself is the
        // answer key, so 2 pair lists are equal iff they contain the
        // same [left, right] pairs, regardless of order.
        "matching" | "word_match" => {
            fn normalize_pairs(pairs: Option<&serde_json::Value>) -> Option<Vec<String>> {
                let arr = pairs?.as_array()?;
                let mut keys = Vec::with_capacity(arr.len());
                for pair in arr {
                    let pair_arr = pair.as_array()?;
                    if pair_arr.len() != 2 {
                        return None;
                    }
                    let left = pair_arr[0].as_str()?;
                    let right = pair_arr[1].as_str()?;
                    keys.push(format!("{left} {right}"));
                }
                keys.sort();
                Some(keys)
            }
            let correct_pairs = normalize_pairs(correct_obj.and_then(|o| o.get("pairs")));
            let submitted_pairs = normalize_pairs(submitted_obj.and_then(|o| o.get("pairs")));
            match (correct_pairs, submitted_pairs) {
                (Some(c), Some(s)) => c == s,
                _ => false,
            }
        }
        "true_false_not_given" => {
            let correct = correct_obj.and_then(|o| o.get("value")).and_then(|v| v.as_str());
            let submitted_value = submitted_obj.and_then(|o| o.get("value")).and_then(|v| v.as_str());
            correct.is_some() && correct == submitted_value
        }
        // Binary — correct only if EVERY passage_id has a matching
        // assignment, same as `matching` above (no partial credit).
        "matching_headings" => {
            let correct_assignments = correct_obj.and_then(|o| o.get("assignments")).and_then(as_object);
            let submitted_assignments = submitted_obj.and_then(|o| o.get("assignments")).and_then(as_object);
            match (correct_assignments, submitted_assignments) {
                (Some(correct), Some(submitted)) => {
                    if correct.is_empty() {
                        return false;
                    }
                    correct.iter().all(|(id, v)| submitted.get(id) == Some(v))
                }
                _ => false,
            }
        }
        // Word-limit engine — an answer exceeding max_words is wrong
        // regardless of text match, checked BEFORE the text comparison.
        "short_answer" => {
            let data_obj = data.and_then(as_object);
            let max_words = data_obj.and_then(|o| o.get("max_words")).and_then(|v| v.as_i64());
            let submitted_text = submitted_obj.and_then(|o| o.get("text")).and_then(|v| v.as_str());
            let (Some(max_words), Some(submitted_text)) = (max_words, submitted_text) else { return false };
            let word_count = submitted_text.trim().split_whitespace().filter(|w| !w.is_empty()).count() as i64;
            if word_count > max_words {
                return false;
            }
            let correct = correct_obj.and_then(|o| o.get("text")).and_then(|v| v.as_str()).map(normalize);
            correct.is_some() && correct == Some(normalize(submitted_text))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mcq_matches_index_only() {
        let correct = json!({"index": 0});
        assert!(is_correct("multiple_choice", &correct, &json!({"index": 0}), None));
        assert!(!is_correct("multiple_choice", &correct, &json!({"index": 1}), None));
        assert!(!is_correct("multiple_choice", &correct, &json!({}), None));
    }

    #[test]
    fn fill_blank_normalizes_case_and_whitespace() {
        let correct = json!({"text": "Jakarta"});
        assert!(is_correct("gap_fill", &correct, &json!({"text": "  jakarta  "}), None));
        assert!(!is_correct("gap_fill", &correct, &json!({"text": "bandung"}), None));
    }

    #[test]
    fn multiple_choice_multiple_is_order_independent_index_set() {
        let correct = json!({"indices": [0, 2]});
        assert!(is_correct("multiple_choice_multiple", &correct, &json!({"indices": [2, 0]}), None));
        assert!(!is_correct("multiple_choice_multiple", &correct, &json!({"indices": [0]}), None));
        assert!(!is_correct("multiple_choice_multiple", &correct, &json!({"indices": []}), None));
    }

    #[test]
    fn sentence_reorder_requires_exact_sequence() {
        let correct = json!({"order": [1, 0, 2]});
        assert!(is_correct("sentence_reorder", &correct, &json!({"order": [1, 0, 2]}), None));
        assert!(!is_correct("sentence_reorder", &correct, &json!({"order": [0, 1, 2]}), None));
    }

    #[test]
    fn true_false_matches_value_only() {
        let correct = json!({"value": "true"});
        assert!(is_correct("true_false", &correct, &json!({"value": "true"}), None));
        assert!(!is_correct("true_false", &correct, &json!({"value": "false"}), None));
    }

    #[test]
    fn matching_is_order_independent_pair_set() {
        let correct = json!({"pairs": [["a", "1"], ["b", "2"]]});
        assert!(is_correct("matching", &correct, &json!({"pairs": [["b", "2"], ["a", "1"]]}), None));
        assert!(!is_correct("matching", &correct, &json!({"pairs": [["a", "1"]]}), None));
    }

    #[test]
    fn matching_headings_requires_every_passage_assigned() {
        let correct = json!({"assignments": {"p1": "h1", "p2": "h2"}});
        assert!(is_correct("matching_headings", &correct, &json!({"assignments": {"p1": "h1", "p2": "h2"}}), None));
        // Partial match on 1 of 2 passages is still wholly wrong — no partial credit.
        assert!(!is_correct("matching_headings", &correct, &json!({"assignments": {"p1": "h1", "p2": "wrong"}}), None));
    }

    #[test]
    fn short_answer_gates_on_word_limit_before_text_match() {
        let correct = json!({"text": "a fine day"});
        let data = json!({"max_words": 3});
        assert!(is_correct("short_answer", &correct, &json!({"text": "a fine day"}), Some(&data)));
        // Exact text match but over the word limit -> still wrong.
        assert!(!is_correct("short_answer", &correct, &json!({"text": "a very fine sunny day"}), Some(&data)));
    }

    #[test]
    fn ai_rubric_and_manual_subtypes_are_never_auto_gradable() {
        // essay/voice_record etc. are AiRubric-graded (quiz_subtype.rs),
        // file_upload/h5p are Manual — neither goes through is_correct.
        assert!(!is_auto_gradable("essay"));
        assert!(!is_auto_gradable("voice_record"));
        assert!(!is_auto_gradable("file_upload"));
        assert!(is_auto_gradable("multiple_choice"));
    }
}
