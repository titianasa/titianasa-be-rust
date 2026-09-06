// Port of grading.ts. Pure functions, no DB access.

pub fn is_auto_gradable(question_type: &str) -> bool {
    matches!(question_type, "mcq" | "fill_blank" | "matching" | "true_false_not_given" | "matching_headings" | "short_answer")
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
        "mcq" => {
            let correct = correct_obj.and_then(|o| o.get("index")).and_then(|v| v.as_i64());
            let submitted_index = submitted_obj.and_then(|o| o.get("index")).and_then(|v| v.as_i64());
            correct.is_some() && correct == submitted_index
        }
        "fill_blank" => {
            let correct = correct_obj.and_then(|o| o.get("text")).and_then(|v| v.as_str()).map(normalize);
            let submitted_text = submitted_obj.and_then(|o| o.get("text")).and_then(|v| v.as_str()).map(normalize);
            correct.is_some() && correct == submitted_text
        }
        // Order-independent set comparison — the pairing itself is the
        // answer key, so 2 pair lists are equal iff they contain the
        // same [left, right] pairs, regardless of order.
        "matching" => {
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
        assert!(is_correct("mcq", &correct, &json!({"index": 0}), None));
        assert!(!is_correct("mcq", &correct, &json!({"index": 1}), None));
        assert!(!is_correct("mcq", &correct, &json!({}), None));
    }

    #[test]
    fn fill_blank_normalizes_case_and_whitespace() {
        let correct = json!({"text": "Jakarta"});
        assert!(is_correct("fill_blank", &correct, &json!({"text": "  jakarta  "}), None));
        assert!(!is_correct("fill_blank", &correct, &json!({"text": "bandung"}), None));
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
    fn writing_and_speaking_are_never_auto_gradable() {
        assert!(!is_auto_gradable("writing"));
        assert!(!is_auto_gradable("speaking"));
        assert!(is_auto_gradable("mcq"));
    }
}
