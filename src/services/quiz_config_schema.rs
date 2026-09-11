// Phase 37 — validates a `module_items.quiz_config` blob against the
// shape in quiz_config.rs.
//
// Split in two, the same "never block a draft save" principle
// content_qa.rs already establishes for articles and questions:
//
// - `validate_structure` (BLOCKING, runs on every autosave from
//   module_item::update_quiz_config) checks only what would otherwise
//   break the engine's ability to dispatch or score a group at all:
//   parseable shape, unique ids, resolvable references, a real registry
//   subtype, and — the one that silently corrupts a whole attempt if it
//   slips through — question numbers unique across the deck, since the
//   learner's answer map is keyed by them.
// - `find_issues` (NON-BLOCKING, runs at submit-review time from
//   content_qa::run_item_qa) reports what isn't finished yet as
//   human-readable strings. A quiz mid-authoring is expected to be
//   incomplete; that is QA feedback, not an error.

use std::collections::HashSet;

use serde_json::Value;

use crate::errors::AppError;
use crate::services::quiz_config::{value_to_key, QuizConfig};
use crate::services::quiz_score;
use crate::services::quiz_subtype;

fn variant_id(variant: quiz_subtype::DisplayVariant) -> &'static str {
    match variant {
        quiz_subtype::DisplayVariant::Default => "default",
        quiz_subtype::DisplayVariant::Ielts => "ielts",
        quiz_subtype::DisplayVariant::Grid => "grid",
    }
}

fn invalid(detail: String) -> AppError {
    AppError::UnprocessableEntity("invalid_quiz_config", detail)
}

/// Parse into the typed config, surfacing serde's message rather than a
/// bare "invalid" so an author can see which field is malformed.
pub fn parse(config: &Value) -> Result<QuizConfig, AppError> {
    serde_json::from_value(config.clone()).map_err(|e| invalid(format!("quiz_config is malformed: {e}")))
}

pub fn validate_structure(config: &Value) -> Result<(), AppError> {
    if !config.is_object() {
        return Err(invalid("quiz_config must be an object".to_string()));
    }
    let parsed = parse(config)?;

    let mut section_ids = HashSet::new();
    for section in &parsed.sections {
        if section.section_id.trim().is_empty() {
            return Err(invalid("each section must have a non-empty section_id".to_string()));
        }
        if !section_ids.insert(section.section_id.clone()) {
            return Err(invalid(format!(r#"duplicate section_id "{}""#, section.section_id)));
        }
    }

    // A section may nest inside another; a dangling or self-referential
    // parent would strand every group under it as unreachable.
    for section in &parsed.sections {
        if let Some(mode) = section.audio.audio_playback_mode.as_deref() {
            if mode != "free" && mode != "exam" {
                return Err(invalid(format!(
                    r#"section "{}": audio_playback_mode must be "free" or "exam""#,
                    section.section_id
                )));
            }
        }
        let Some(parent) = &section.parent_section_id else { continue };
        if parent == &section.section_id {
            return Err(invalid(format!(r#"section "{}" cannot be its own parent"#, section.section_id)));
        }
        if !section_ids.contains(parent) {
            return Err(invalid(format!(
                r#"section "{}"'s parent_section_id "{parent}" is not one of sections' ids"#,
                section.section_id
            )));
        }
    }
    detect_section_cycle(&parsed)?;

    let mut group_ids = HashSet::new();
    // Question numbers key the learner's answer map, so a collision
    // anywhere in the deck silently makes two questions share one answer.
    let mut question_keys: HashSet<String> = HashSet::new();

    for group in &parsed.question_groups {
        if group.group_id.trim().is_empty() {
            return Err(invalid("each question_group must have a non-empty group_id".to_string()));
        }
        if !group_ids.insert(group.group_id.clone()) {
            return Err(invalid(format!(r#"duplicate group_id "{}""#, group.group_id)));
        }
        if let Some(section_id) = &group.section_id {
            if !section_ids.contains(section_id) {
                return Err(invalid(format!(
                    r#"question_group "{}"'s section_id "{section_id}" is not one of sections' ids"#,
                    group.group_id
                )));
            }
        }
        let Some(info) = quiz_subtype::find(&group.r#type) else {
            return Err(invalid(format!(r#"question_group "{}": unknown subtype "{}""#, group.group_id, group.r#type)));
        };
        // A presentation variant only means something for the subtypes
        // that implement it — asking for a drag-and-drop word bank on an
        // essay would render as a plain essay and quietly mislead the
        // author about what the learner will see.
        if let Some(mode) = group.display_mode.as_deref().filter(|m| !m.is_empty() && *m != "default") {
            let supported = info.variants.iter().any(|v| variant_id(*v) == mode);
            if !supported {
                let allowed: Vec<&str> = std::iter::once("default").chain(info.variants.iter().map(|v| variant_id(*v))).collect();
                return Err(invalid(format!(
                    r#"question_group "{}": subtype "{}" does not support display_mode "{mode}" (allowed: {})"#,
                    group.group_id,
                    group.r#type,
                    allowed.join(", ")
                )));
            }
        }

        if let Some(mode) = group.audio.audio_playback_mode.as_deref() {
            if mode != "free" && mode != "exam" {
                return Err(invalid(format!(
                    r#"question_group "{}": audio_playback_mode must be "free" or "exam""#,
                    group.group_id
                )));
            }
        }

        for question in &group.questions {
            if matches!(question.number, Value::Null) {
                return Err(invalid(format!(r#"question_group "{}": every question must have a number"#, group.group_id)));
            }
            let key = value_to_key(&question.number);
            if key.trim().is_empty() {
                return Err(invalid(format!(r#"question_group "{}": every question must have a non-empty number"#, group.group_id)));
            }
            if !question_keys.insert(key.clone()) {
                return Err(invalid(format!(
                    r#"duplicate question number "{key}" (in question_group "{}") — numbers must be unique across the whole quiz"#,
                    group.group_id
                )));
            }
        }
    }

    Ok(())
}

/// Walk each section up to the root; a cycle would otherwise hang any
/// renderer that builds the section tree.
fn detect_section_cycle(config: &QuizConfig) -> Result<(), AppError> {
    for section in &config.sections {
        let mut seen = HashSet::new();
        seen.insert(section.section_id.clone());
        let mut cursor = section.parent_section_id.clone();
        while let Some(parent_id) = cursor {
            if !seen.insert(parent_id.clone()) {
                return Err(invalid(format!(r#"sections form a cycle at "{parent_id}""#)));
            }
            cursor = config.find_section(&parent_id).and_then(|s| s.parent_section_id.clone());
        }
    }
    Ok(())
}

/// Non-blocking QA. One string per thing that isn't finished yet; an
/// empty vec means the quiz is ready to publish.
pub fn find_issues(config: &Value) -> Vec<String> {
    let Ok(parsed) = parse(config) else {
        return vec!["quiz_config tidak bisa dibaca".to_string()];
    };

    let mut issues = Vec::new();

    if parsed.question_groups.is_empty() {
        issues.push("kuis belum punya satu grup soal pun".to_string());
    }

    for group in &parsed.question_groups {
        let id = &group.group_id;
        let subtype = &group.r#type;
        let Some(info) = quiz_subtype::find(subtype) else { continue };

        if group.questions.is_empty() {
            issues.push(format!(r#"grup "{id}" ({subtype}) belum punya soal"#));
            continue;
        }

        // A reading/listening subtype with nothing to read or listen to
        // is unanswerable — but only flag it once the group actually has
        // questions, and only when neither the group, its section, nor
        // the deck supplies the context.
        if info.needs_passage && resolved_passage(&parsed, group).is_none() {
            issues.push(format!(r#"grup "{id}" ({subtype}) butuh passage tapi belum ada"#));
        }
        if info.needs_audio && resolved_audio(&parsed, group).is_none() {
            issues.push(format!(r#"grup "{id}" ({subtype}) butuh audio tapi belum ada"#));
        }

        for question in &group.questions {
            let number = value_to_key(&question.number);
            if question.prompt_text().is_none_or(str::is_empty) {
                issues.push(format!(r#"grup "{id}" soal {number}: belum ada pertanyaan/instruksi"#));
            }
            // A deferred-grading subtype has no answer key by design —
            // asking for one would flag every essay in the deck.
            if !quiz_score::needs_evaluation(subtype) {
                let has_answer = question.answer.as_ref().is_some_and(|a| !answer_is_empty(a)) || !question.answers.is_empty();
                if !has_answer {
                    issues.push(format!(r#"grup "{id}" soal {number}: belum ada kunci jawaban"#));
                }
            }
        }
    }

    issues
}

fn answer_is_empty(answer: &Value) -> bool {
    match answer {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

/// A group inherits its section's passage, and the section the deck's,
/// when its own slot is empty.
fn resolved_passage<'a>(config: &'a QuizConfig, group: &'a crate::services::quiz_config::QuizQuestionGroup) -> Option<&'a str> {
    let own = group.passage.as_deref().filter(|s| !s.trim().is_empty());
    if own.is_some() {
        return own;
    }
    if !group.passage_sections.is_empty() {
        return Some("");
    }
    let from_section = group
        .section_id
        .as_deref()
        .and_then(|id| config.find_section(id))
        .and_then(|s| s.passage.as_deref())
        .filter(|s| !s.trim().is_empty());
    from_section.or_else(|| config.passage.as_deref().filter(|s| !s.trim().is_empty()))
}

fn resolved_audio<'a>(config: &'a QuizConfig, group: &'a crate::services::quiz_config::QuizQuestionGroup) -> Option<&'a str> {
    let own = group.audio.audio_url.as_deref().filter(|s| !s.trim().is_empty());
    if own.is_some() {
        return own;
    }
    let from_section = group
        .section_id
        .as_deref()
        .and_then(|id| config.find_section(id))
        .and_then(|s| s.audio.audio_url.as_deref())
        .filter(|s| !s.trim().is_empty());
    from_section.or_else(|| config.audio.audio_url.as_deref().filter(|s| !s.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_one_passage_answered_by_several_questions() {
        let config = json!({
            "sections": [{"section_id": "s1", "title": "Reading Passage 1"}],
            "question_groups": [{
                "group_id": "g1",
                "type": "true_false_not_given",
                "section_id": "s1",
                "passage": "Coral reefs cover less than 1% of the ocean floor.",
                "questions": [
                    {"number": 1, "text": "Reefs cover under 1%.", "answer": "True"},
                    {"number": 2, "text": "Reefs are Pacific-only.", "answer": "Not Given"},
                ],
            }],
        });
        assert!(validate_structure(&config).is_ok());
        assert!(find_issues(&config).is_empty(), "issues: {:?}", find_issues(&config));
    }

    #[test]
    fn rejects_a_duplicate_question_number_across_groups() {
        // The corruption this exists to stop: two questions sharing one
        // answer-map key, so answering one silently answers the other.
        let config = json!({
            "sections": [],
            "question_groups": [
                {"group_id": "g1", "type": "multiple_choice", "questions": [{"number": 1, "stem": "a", "answer": "A"}]},
                {"group_id": "g2", "type": "short_answer", "questions": [{"number": 1, "stem": "b", "answer": "x"}]},
            ],
        });
        let err = validate_structure(&config).unwrap_err();
        assert!(format!("{err:?}").contains("duplicate question number"));
    }

    #[test]
    fn rejects_unknown_subtype_and_dangling_section_id() {
        let unknown = json!({
            "sections": [{"section_id": "s1", "title": "S"}],
            "question_groups": [{"group_id": "g1", "type": "not_a_real_subtype", "section_id": "s1", "questions": []}],
        });
        assert!(validate_structure(&unknown).is_err());

        let dangling = json!({
            "sections": [{"section_id": "s1", "title": "S"}],
            "question_groups": [{"group_id": "g1", "type": "essay", "section_id": "nope", "questions": []}],
        });
        assert!(validate_structure(&dangling).is_err());
    }

    #[test]
    fn accepts_a_display_variant_the_subtype_supports() {
        let config = json!({
            "sections": [],
            "question_groups": [{
                "group_id": "g1", "type": "matching", "display_mode": "grid",
                "options": ["A", "B"],
                "questions": [{"number": 1, "label": "Paragraf 1", "answer": "A"}],
            }],
        });
        assert!(validate_structure(&config).is_ok());
    }

    #[test]
    fn rejects_a_display_variant_the_subtype_does_not_implement() {
        // An essay rendered as a "drag-and-drop word bank" would silently
        // fall back to a plain textarea; better to refuse it outright.
        let config = json!({
            "sections": [],
            "question_groups": [{
                "group_id": "g1", "type": "essay", "display_mode": "ielts",
                "questions": [{"number": 1, "prompt": "Tulis esai."}],
            }],
        });
        let err = validate_structure(&config).unwrap_err();
        assert!(format!("{err:?}").contains("does not support display_mode"));
    }

    #[test]
    fn grid_is_matching_only() {
        let config = json!({
            "sections": [],
            "question_groups": [{
                "group_id": "g1", "type": "gap_fill", "display_mode": "grid",
                "questions": [{"number": 1, "stem": "___", "answer": "x"}],
            }],
        });
        assert!(validate_structure(&config).is_err());
    }

    #[test]
    fn rejects_an_unknown_audio_playback_mode() {
        let config = json!({
            "sections": [{"section_id": "s1", "title": "S", "audio_playback_mode": "kiosk"}],
            "question_groups": [],
        });
        assert!(validate_structure(&config).is_err());
    }

    #[test]
    fn accepts_exam_playback_mode() {
        let config = json!({
            "sections": [{"section_id": "s1", "title": "S", "audio_url": "https://x/a.mp3", "audio_playback_mode": "exam"}],
            "question_groups": [],
        });
        assert!(validate_structure(&config).is_ok());
    }

    #[test]
    fn rejects_a_section_cycle() {
        let config = json!({
            "sections": [
                {"section_id": "a", "parent_section_id": "b"},
                {"section_id": "b", "parent_section_id": "a"},
            ],
            "question_groups": [],
        });
        assert!(validate_structure(&config).is_err());
    }

    #[test]
    fn allows_a_nested_section_tree() {
        let config = json!({
            "sections": [
                {"section_id": "root", "title": "Listening"},
                {"section_id": "part1", "parent_section_id": "root", "title": "Part 1"},
            ],
            "question_groups": [{
                "group_id": "g1", "type": "short_answer", "section_id": "part1",
                "audio_url": "https://example.test/a.mp3",
                "questions": [{"number": 1, "stem": "What time?", "answer": "9am"}],
            }],
        });
        assert!(validate_structure(&config).is_ok());
    }

    #[test]
    fn structure_still_saves_an_unfinished_quiz() {
        // Autosave must never refuse a draft; the gaps come back as QA.
        let config = json!({
            "sections": [{"section_id": "s1", "title": "S"}],
            "question_groups": [{
                "group_id": "g1", "type": "multiple_choice", "section_id": "s1",
                "questions": [{"number": 1}],
            }],
        });
        assert!(validate_structure(&config).is_ok());
        let issues = find_issues(&config);
        assert!(issues.iter().any(|i| i.contains("pertanyaan")), "issues: {issues:?}");
        assert!(issues.iter().any(|i| i.contains("kunci jawaban")), "issues: {issues:?}");
    }

    #[test]
    fn an_essay_is_not_asked_for_an_answer_key() {
        let config = json!({
            "sections": [],
            "question_groups": [{
                "group_id": "g1", "type": "essay",
                "questions": [{"number": 1, "prompt": "Describe your weekend.", "min_words": 150}],
            }],
        });
        assert!(validate_structure(&config).is_ok());
        assert!(find_issues(&config).is_empty(), "issues: {:?}", find_issues(&config));
    }

    #[test]
    fn a_group_inherits_its_sections_passage_before_being_flagged() {
        let config = json!({
            "sections": [{"section_id": "s1", "title": "S", "passage": "A long shared passage."}],
            "question_groups": [{
                "group_id": "g1", "type": "matching_headings", "section_id": "s1",
                "headings": [{"label": "i", "text": "A heading"}],
                "questions": [{"number": 1, "label": "Paragraph A", "answer": "A heading"}],
            }],
        });
        let issues = find_issues(&config);
        assert!(!issues.iter().any(|i| i.contains("passage")), "issues: {issues:?}");
    }

    #[test]
    fn an_empty_group_is_flagged_but_still_saves() {
        let config = json!({
            "sections": [],
            "question_groups": [{"group_id": "g1", "type": "multiple_choice", "questions": []}],
        });
        assert!(validate_structure(&config).is_ok());
        assert!(find_issues(&config).iter().any(|i| i.contains("belum punya soal")));
    }
}
