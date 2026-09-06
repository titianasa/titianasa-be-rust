use serde_json::Value;

use crate::errors::AppError;

// Port of block_schema.ts. P2-003: registry of content_blocks.type
// validators. Structural validation only; question_embed's referential
// check (does question_id actually exist) needs a DB round-trip and
// lives one layer up in content_block.rs.

const UUID_PATTERN_LEN: usize = 36;

fn is_uuid(s: &str) -> bool {
    if s.len() != UUID_PATTERN_LEN {
        return false;
    }
    uuid::Uuid::parse_str(s).is_ok()
}

fn invalid(detail: String) -> AppError {
    AppError::UnprocessableEntity("invalid_block_schema", detail)
}

fn as_object<'a>(data: &'a Value, type_name: &str) -> Result<&'a serde_json::Map<String, Value>, AppError> {
    data.as_object().ok_or_else(|| invalid(format!("type={type_name}: data must be an object")))
}

fn require_non_empty_string(obj: &serde_json::Map<String, Value>, field: &str, type_name: &str) -> Result<(), AppError> {
    let value = obj.get(field).and_then(|v| v.as_str());
    match value {
        Some(s) if !s.trim().is_empty() => Ok(()),
        Some(_) => Err(invalid(format!("type={type_name}: data.{field} must not be empty"))),
        None => Err(invalid(format!("type={type_name}: data.{field} must be a string"))),
    }
}

fn reject_unknown_fields(obj: &serde_json::Map<String, Value>, allowed: &[&str], type_name: &str) -> Result<(), AppError> {
    let unknown: Vec<&str> = obj.keys().map(|k| k.as_str()).filter(|k| !allowed.contains(k)).collect();
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown field for type={type_name}: {}", unknown.join(", "))));
    }
    Ok(())
}

fn validate_asset_reference(obj: &serde_json::Map<String, Value>, type_name: &str) -> Result<(), AppError> {
    let asset = obj.get("asset").and_then(|v| v.as_str());
    match asset {
        Some(a) if a.starts_with("asset://") => Ok(()),
        Some(_) => Err(invalid(format!(
            r#"type={type_name}: data.asset must be an "asset://" reference, not raw content — see ADR-0001/2.1"#
        ))),
        None => Err(invalid(format!("type={type_name}: data.asset must be a string"))),
    }
}

pub fn validate(r#type: &str, data: &Value) -> Result<(), AppError> {
    match r#type {
        "text" => {
            let obj = as_object(data, "text")?;
            require_non_empty_string(obj, "text", "text")?;
            reject_unknown_fields(obj, &["text"], "text")
        }
        "heading" => {
            let obj = as_object(data, "heading")?;
            require_non_empty_string(obj, "text", "heading")?;
            if let Some(level) = obj.get("level") {
                let level_num = level.as_i64();
                match level_num {
                    Some(n) if (1..=6).contains(&n) => {}
                    Some(_) => return Err(invalid("type=heading: data.level must be 1-6".to_string())),
                    None => return Err(invalid("type=heading: data.level must be an integer".to_string())),
                }
            }
            reject_unknown_fields(obj, &["text", "level"], "heading")
        }
        "example" => {
            let obj = as_object(data, "example")?;
            require_non_empty_string(obj, "text", "example")?;
            reject_unknown_fields(obj, &["text"], "example")
        }
        "audio" => {
            let obj = as_object(data, "audio")?;
            validate_asset_reference(obj, "audio")?;
            reject_unknown_fields(obj, &["asset", "title"], "audio")
        }
        "video" => {
            let obj = as_object(data, "video")?;
            validate_asset_reference(obj, "video")?;
            reject_unknown_fields(obj, &["asset", "title"], "video")
        }
        "image" => {
            let obj = as_object(data, "image")?;
            validate_asset_reference(obj, "image")?;
            reject_unknown_fields(obj, &["asset", "alt"], "image")
        }
        "flashcard" => {
            let obj = as_object(data, "flashcard")?;
            require_non_empty_string(obj, "front", "flashcard")?;
            require_non_empty_string(obj, "back", "flashcard")?;
            reject_unknown_fields(obj, &["front", "back"], "flashcard")
        }
        "question_embed" => {
            let obj = as_object(data, "question_embed")?;
            let question_id = obj.get("question_id").and_then(|v| v.as_str());
            match question_id {
                Some(id) if is_uuid(id) => {}
                Some(_) => return Err(invalid("type=question_embed: data.question_id must be a valid UUID".to_string())),
                None => return Err(invalid("type=question_embed: data.question_id must be a string".to_string())),
            }
            reject_unknown_fields(obj, &["question_id"], "question_embed")
        }
        "assessment_embed" => {
            let obj = as_object(data, "assessment_embed")?;
            let assessment_id = obj.get("assessment_id").and_then(|v| v.as_str());
            match assessment_id {
                Some(id) if is_uuid(id) => {}
                Some(_) => return Err(invalid("type=assessment_embed: data.assessment_id must be a valid UUID".to_string())),
                None => return Err(invalid("type=assessment_embed: data.assessment_id must be a string".to_string())),
            }
            reject_unknown_fields(obj, &["assessment_id"], "assessment_embed")
        }
        "indonesian_learner_alert" => {
            let obj = as_object(data, "indonesian_learner_alert")?;
            require_non_empty_string(obj, "text", "indonesian_learner_alert")?;
            reject_unknown_fields(obj, &["text"], "indonesian_learner_alert")
        }
        "common_trap" => {
            let obj = as_object(data, "common_trap")?;
            require_non_empty_string(obj, "term", "common_trap")?;
            require_non_empty_string(obj, "explanation", "common_trap")?;
            reject_unknown_fields(obj, &["term", "explanation"], "common_trap")
        }
        "think_in_english" => {
            let obj = as_object(data, "think_in_english")?;
            require_non_empty_string(obj, "indonesian_pattern", "think_in_english")?;
            require_non_empty_string(obj, "english_pattern", "think_in_english")?;
            reject_unknown_fields(obj, &["indonesian_pattern", "english_pattern"], "think_in_english")
        }
        "speaking_prompt" => {
            let obj = as_object(data, "speaking_prompt")?;
            require_non_empty_string(obj, "text", "speaking_prompt")?;
            reject_unknown_fields(obj, &["text"], "speaking_prompt")
        }
        other => Err(invalid(format!(r#"unsupported block type "{other}" — not registered in BlockTypeRegistry"#))),
    }
}

pub struct BlockRef<'a> {
    pub r#type: &'a str,
    pub data: &'a Value,
}

fn extract_embed_ids(blocks: &[BlockRef], block_type: &str, field: &str) -> Vec<String> {
    blocks
        .iter()
        .filter(|b| b.r#type == block_type)
        .filter_map(|b| b.data.as_object())
        .filter_map(|obj| obj.get(field).and_then(|v| v.as_str()))
        .filter(|id| is_uuid(id))
        .map(|s| s.to_string())
        .collect()
}

pub fn extract_question_embed_ids(blocks: &[BlockRef]) -> Vec<String> {
    extract_embed_ids(blocks, "question_embed", "question_id")
}

pub fn extract_assessment_embed_ids(blocks: &[BlockRef]) -> Vec<String> {
    extract_embed_ids(blocks, "assessment_embed", "assessment_id")
}
