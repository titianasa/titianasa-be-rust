use serde_json::Value;

use crate::errors::AppError;

// Port of question_schema.ts. P2-004: question_type is a *registered
// component type*, not a hardcoded enum — adding a new type means
// adding one registry entry here, mirroring block_schema.rs's idiom.

fn invalid(detail: String) -> AppError {
    AppError::UnprocessableEntity("invalid_question_schema", detail)
}

fn as_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object()
}

fn unknown_fields<'a>(obj: &'a serde_json::Map<String, Value>, allowed: &[&str]) -> Vec<&'a str> {
    obj.keys().map(|k| k.as_str()).filter(|k| !allowed.contains(k)).collect()
}

pub fn validate(r#type: &str, data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    match r#type {
        // Phase 37 renames: mcq -> multiple_choice, fill_blank -> gap_fill
        // (see migrations/0038's questions.type backfill) — aligned to
        // quiz_subtype.rs's registry, which shares parelabs-backend's
        // SubtypeId vocabulary.
        "multiple_choice" => validate_mcq(data, correct_answer),
        "gap_fill" => validate_fill_blank(data, correct_answer),
        "matching" => validate_matching(data, correct_answer),
        "true_false_not_given" => validate_true_false_not_given(data, correct_answer),
        "matching_headings" => validate_matching_headings(data, correct_answer),
        "short_answer" => validate_short_answer(data, correct_answer),
        // Shared-primitive subtypes (Phase 37) — many `Auto`-graded
        // quiz_subtype.rs entries are structurally identical to one of
        // a handful of shapes, so they delegate to one validator per
        // shape rather than each getting a bespoke one. See
        // grading.rs's is_correct for the matching primitive on the
        // grading side.
        "multiple_choice_multiple" => validate_multi_index(r#type, data, correct_answer),
        "true_false" => validate_enum_value(r#type, data, correct_answer, &["true", "false"]),
        "yes_no_not_given" => validate_enum_value(r#type, data, correct_answer, &["yes", "no", "not_given"]),
        "minimal_pairs" | "stress_pattern" | "intonation" => validate_mcq(data, correct_answer),
        "word_form" | "sentence_transform" | "vocab_cloze" | "spelling" | "image_word" | "analogy" | "cloze_passage" | "error_correction" => {
            validate_text_match(r#type, data, correct_answer)
        }
        "word_match" => validate_matching(data, correct_answer),
        "table_completion" | "flow_chart" | "map_labeling" => validate_assignment_map(r#type, data, correct_answer),
        "sentence_reorder" | "word_scramble" => validate_ordered_list(r#type, data, correct_answer),
        "highlight_incorrect_words" | "error_identification" => validate_span_selection(r#type, data, correct_answer),
        other => Err(invalid(format!(r#"unsupported type "{other}" — not registered in QuestionTypeRegistry"#))),
    }
}

fn validate_multi_index(r#type: &str, data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type={type}: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(prompt) = obj.get("prompt").and_then(|v| v.as_str()) else { return err("data.prompt must be a string") };
    if prompt.trim().is_empty() {
        return err("data.prompt must not be empty");
    }
    let Some(options) = obj.get("options").and_then(|v| v.as_array()) else { return err("data.options must be an array") };
    if options.len() < 2 || !options.iter().all(|o| o.is_string()) {
        return err("data.options must be an array of at least 2 strings");
    }
    let unknown = unknown_fields(obj, &["prompt", "options"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type={type}: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(indices) = answer_obj.get("indices").and_then(|v| v.as_array()) else {
        return err("correct_answer.indices must be a non-empty array of indices");
    };
    if indices.is_empty() || !indices.iter().all(|i| i.as_i64().is_some_and(|i| i >= 0 && (i as usize) < options.len())) {
        return err("correct_answer.indices must all be valid, non-negative indices into data.options");
    }
    Ok(())
}

fn validate_enum_value(r#type: &str, data: &Value, correct_answer: &Value, allowed: &[&str]) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type={type}: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(statement) = obj.get("statement").and_then(|v| v.as_str()) else { return err("data.statement must be a string") };
    if statement.trim().is_empty() {
        return err("data.statement must not be empty");
    }
    let unknown = unknown_fields(obj, &["statement"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type={type}: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let value = answer_obj.get("value").and_then(|v| v.as_str());
    if !value.is_some_and(|v| allowed.contains(&v)) {
        return Err(invalid(format!(r#"type={type}: correct_answer.value must be one of {allowed:?}"#)));
    }
    Ok(())
}

// Mirrors validate_fill_blank exactly — same {prompt}/{text} shape,
// just shared across every subtype whose answer is "one normalized
// piece of text" (a word, a corrected sentence, a whole passage).
fn validate_text_match(r#type: &str, data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type={type}: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(prompt) = obj.get("prompt").and_then(|v| v.as_str()) else { return err("data.prompt must be a string") };
    if prompt.trim().is_empty() {
        return err("data.prompt must not be empty");
    }
    let unknown = unknown_fields(obj, &["prompt"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type={type}: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(text) = answer_obj.get("text").and_then(|v| v.as_str()) else { return err("correct_answer.text must be a string") };
    if text.trim().is_empty() {
        return err("correct_answer.text must not be empty");
    }
    Ok(())
}

// Generalized matching_headings shape (`items`/`options` instead of
// `passages`/`headings`) for the other assignment-style subtypes —
// matching_headings itself keeps its own dedicated validator/grader
// unchanged (real existing renderer depends on those exact field
// names), this is for the newer siblings only.
fn validate_assignment_map(r#type: &str, data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type={type}: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(items) = obj.get("items").and_then(|v| v.as_array()) else {
        return err("data.items must be an array of {id, label} with at least 2 entries");
    };
    if items.len() < 2 {
        return err("data.items must have at least 2 entries");
    }
    let mut item_ids = std::collections::HashSet::new();
    for item in items {
        let Some(item_obj) = as_object(item) else { return err("each entry in data.items must be {id: string, label: string}") };
        let (Some(id), Some(_label)) = (item_obj.get("id").and_then(|v| v.as_str()), item_obj.get("label").and_then(|v| v.as_str())) else {
            return err("each entry in data.items must be {id: string, label: string}");
        };
        if !item_ids.insert(id.to_string()) {
            return Err(invalid(format!(r#"type={type}: duplicate item id "{id}""#)));
        }
    }
    let Some(options) = obj.get("options").and_then(|v| v.as_array()) else {
        return err("data.options must be an array with at least as many entries as data.items");
    };
    if options.len() < items.len() || !options.iter().all(|o| o.is_string()) {
        return err("data.options must be an array of strings, at least as long as data.items");
    }
    let unknown = unknown_fields(obj, &["items", "options"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type={type}: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(assignments) = answer_obj.get("assignments").and_then(as_object) else {
        return err("correct_answer.assignments must be an object");
    };
    for item_id in &item_ids {
        let option_index = assignments.get(item_id).and_then(|v| v.as_i64());
        let valid = option_index.is_some_and(|i| i >= 0 && (i as usize) < options.len());
        if !valid {
            return Err(invalid(format!(r#"type={type}: correct_answer.assignments["{item_id}"] must be a valid index into data.options"#)));
        }
    }
    Ok(())
}

// data.tokens are shown to the learner in a shuffled/base order; the
// answer is the permutation of their indices that reads correctly.
fn validate_ordered_list(r#type: &str, data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type={type}: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(tokens) = obj.get("tokens").and_then(|v| v.as_array()) else { return err("data.tokens must be an array of at least 2 strings") };
    if tokens.len() < 2 || !tokens.iter().all(|t| t.is_string()) {
        return err("data.tokens must be an array of at least 2 strings");
    }
    let unknown = unknown_fields(obj, &["prompt", "tokens"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type={type}: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(order) = answer_obj.get("order").and_then(|v| v.as_array()) else {
        return err("correct_answer.order must be a permutation of data.tokens' indices");
    };
    let mut seen = std::collections::HashSet::new();
    let valid = order.len() == tokens.len()
        && order.iter().all(|i| {
            i.as_i64().is_some_and(|i| i >= 0 && (i as usize) < tokens.len() && seen.insert(i))
        });
    if !valid {
        return Err(invalid(format!("type={type}: correct_answer.order must be a permutation of data.tokens' indices")));
    }
    Ok(())
}

// data.tokens are shown to the learner (e.g. every word in a
// sentence); the answer is the SET of token indices that are the
// target (e.g. the incorrect ones to highlight).
fn validate_span_selection(r#type: &str, data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type={type}: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(tokens) = obj.get("tokens").and_then(|v| v.as_array()) else { return err("data.tokens must be an array of at least 2 strings") };
    if tokens.len() < 2 || !tokens.iter().all(|t| t.is_string()) {
        return err("data.tokens must be an array of at least 2 strings");
    }
    let unknown = unknown_fields(obj, &["text", "tokens"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type={type}: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(indices) = answer_obj.get("indices").and_then(|v| v.as_array()) else {
        return err("correct_answer.indices must be a non-empty array of indices");
    };
    if indices.is_empty() || !indices.iter().all(|i| i.as_i64().is_some_and(|i| i >= 0 && (i as usize) < tokens.len())) {
        return err("correct_answer.indices must all be valid, non-negative indices into data.tokens");
    }
    Ok(())
}

fn validate_mcq(data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type=mcq: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(prompt) = obj.get("prompt").and_then(|v| v.as_str()) else { return err("data.prompt must be a string") };
    if prompt.trim().is_empty() {
        return err("data.prompt must not be empty");
    }
    let Some(options) = obj.get("options").and_then(|v| v.as_array()) else { return err("data.options must be an array") };
    if options.len() < 2 {
        return err("data.options must have at least 2 entries");
    }
    if !options.iter().all(|o| o.is_string()) {
        return err("data.options must all be strings");
    }
    let unknown = unknown_fields(obj, &["prompt", "options"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type=mcq: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(index) = answer_obj.get("index").and_then(|v| v.as_i64()) else {
        return err("correct_answer.index must be a non-negative integer");
    };
    if index < 0 {
        return err("correct_answer.index must be a non-negative integer");
    }
    if index as usize >= options.len() {
        return err("correct_answer.index is out of range for data.options");
    }
    Ok(())
}

fn validate_fill_blank(data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type=fill_blank: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(prompt) = obj.get("prompt").and_then(|v| v.as_str()) else { return err("data.prompt must be a string") };
    if prompt.trim().is_empty() {
        return err("data.prompt must not be empty");
    }
    let unknown = unknown_fields(obj, &["prompt"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type=fill_blank: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(text) = answer_obj.get("text").and_then(|v| v.as_str()) else { return err("correct_answer.text must be a string") };
    if text.trim().is_empty() {
        return err("correct_answer.text must not be empty");
    }
    Ok(())
}

fn validate_matching(data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type=matching: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(pairs) = obj.get("pairs").and_then(|v| v.as_array()) else { return err("data.pairs must be an array") };
    if pairs.len() < 2 {
        return err("data.pairs must have at least 2 entries");
    }
    for pair in pairs {
        let ok = pair.as_array().map(|p| p.len() == 2 && p.iter().all(|x| x.is_string())).unwrap_or(false);
        if !ok {
            return err("each entry in data.pairs must be exactly [left: string, right: string]");
        }
    }
    let unknown = unknown_fields(obj, &["pairs"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type=matching: {}", unknown.join(", "))));
    }

    // correct_answer for `matching` is redundant with data.pairs — still
    // required to be present and object-shaped, so grading has a stable
    // place to read from without special-casing this one type.
    if as_object(correct_answer).is_none() {
        return err("correct_answer must be an object");
    }
    Ok(())
}

fn validate_true_false_not_given(data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type=true_false_not_given: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(statement) = obj.get("statement").and_then(|v| v.as_str()) else { return err("data.statement must be a string") };
    if statement.trim().is_empty() {
        return err("data.statement must not be empty");
    }
    let unknown = unknown_fields(obj, &["statement"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type=true_false_not_given: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let value = answer_obj.get("value").and_then(|v| v.as_str());
    if !matches!(value, Some("true") | Some("false") | Some("not_given")) {
        return err(r#"correct_answer.value must be one of "true", "false", "not_given""#);
    }
    Ok(())
}

fn validate_matching_headings(data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type=matching_headings: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(passages) = obj.get("passages").and_then(|v| v.as_array()) else {
        return err("data.passages must be an array with at least 2 entries");
    };
    if passages.len() < 2 {
        return err("data.passages must be an array with at least 2 entries");
    }
    let mut passage_ids = std::collections::HashSet::new();
    for p in passages {
        let Some(passage) = as_object(p) else { return err("each entry in data.passages must be {id: string, text: string}") };
        let (Some(id), Some(_text)) = (passage.get("id").and_then(|v| v.as_str()), passage.get("text").and_then(|v| v.as_str())) else {
            return err("each entry in data.passages must be {id: string, text: string}");
        };
        if !passage_ids.insert(id.to_string()) {
            return Err(invalid(format!(r#"duplicate passage id "{id}""#)));
        }
    }
    let Some(headings) = obj.get("headings").and_then(|v| v.as_array()) else {
        return err("data.headings must be an array with at least as many entries as data.passages");
    };
    if headings.len() < passages.len() {
        return err("data.headings must be an array with at least as many entries as data.passages");
    }
    if !headings.iter().all(|h| h.is_string()) {
        return err("data.headings must all be strings");
    }
    let unknown = unknown_fields(obj, &["passages", "headings"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type=matching_headings: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(assignments) = answer_obj.get("assignments").and_then(as_object) else {
        return err("correct_answer.assignments must be an object");
    };
    for passage_id in &passage_ids {
        let heading_index = assignments.get(passage_id).and_then(|v| v.as_i64());
        let valid = heading_index.map(|i| i >= 0 && (i as usize) < headings.len()).unwrap_or(false);
        if !valid {
            return Err(invalid(format!(r#"correct_answer.assignments["{passage_id}"] must be a valid index into data.headings"#)));
        }
    }
    Ok(())
}

fn validate_short_answer(data: &Value, correct_answer: &Value) -> Result<(), AppError> {
    let err = |msg: &str| Err(invalid(format!("type=short_answer: {msg}")));
    let Some(obj) = as_object(data) else { return err("data must be an object") };
    let Some(prompt) = obj.get("prompt").and_then(|v| v.as_str()) else { return err("data.prompt must be a string") };
    if prompt.trim().is_empty() {
        return err("data.prompt must not be empty");
    }
    let Some(max_words) = obj.get("max_words").and_then(|v| v.as_i64()) else { return err("data.max_words must be a positive integer") };
    if max_words < 1 {
        return err("data.max_words must be a positive integer");
    }
    let unknown = unknown_fields(obj, &["prompt", "max_words"]);
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type=short_answer: {}", unknown.join(", "))));
    }

    let Some(answer_obj) = as_object(correct_answer) else { return err("correct_answer must be an object") };
    let Some(text) = answer_obj.get("text").and_then(|v| v.as_str()) else { return err("correct_answer.text must be a string") };
    if text.trim().is_empty() {
        return err("correct_answer.text must not be empty");
    }
    Ok(())
}
