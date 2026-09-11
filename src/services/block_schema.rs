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

// A media block points at one of two things:
//
//   - `asset://<id>` — a file in our own Drive. It MUST stay a reference:
//     the underlying R2 url is signed and expires (1h private / 7d
//     public), so a stored url would go stale. This is the rule
//     ADR-0001/2.1 set, and it still holds.
//   - an `https://` link to a third party (Google Drive, YouTube, Vimeo,
//     a CDN). These don't expire, so there is nothing to go stale, and
//     rejecting them only meant an author could not use the source their
//     material actually lives on. The frontend's `media-url.ts`
//     translates each provider's share link into its playable form.
//
// Exactly one of `asset` / `src` is expected; both empty is a block
// that renders nothing.
fn validate_media_source(obj: &serde_json::Map<String, Value>, type_name: &str) -> Result<(), AppError> {
    let asset = obj.get("asset").and_then(|v| v.as_str()).filter(|a| !a.trim().is_empty());
    let src = obj.get("src").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty());

    match (asset, src) {
        (Some(a), _) if !a.starts_with("asset://") => Err(invalid(format!(
            r#"type={type_name}: data.asset must be an "asset://" reference — put an external link in data.src instead"#
        ))),
        (Some(_), _) => Ok(()),
        (None, Some(s)) if s.starts_with("http://") || s.starts_with("https://") => Ok(()),
        (None, Some(_)) => Err(invalid(format!("type={type_name}: data.src must be an http(s) url"))),
        (None, None) => Err(invalid(format!("type={type_name}: needs either data.asset or data.src"))),
    }
}

/// Presentation-only fields every media block shares.
fn validate_media_display(obj: &serde_json::Map<String, Value>, type_name: &str) -> Result<(), AppError> {
    if let Some(width) = obj.get("width") {
        let allowed = ["full", "large", "medium", "small"];
        match width.as_str() {
            Some(w) if allowed.contains(&w) => {}
            _ => return Err(invalid(format!("type={type_name}: data.width must be one of [{}]", allowed.join(", ")))),
        }
    }
    Ok(())
}

// The structured blocks (table rows, list items, timeline entries) carry
// real arrays rather than delimiter-joined strings — see alm_parser's
// `parse_scalar`. These check that shape without demanding every cell be
// filled in, since a half-built table is normal while authoring.
fn require_string_array(obj: &serde_json::Map<String, Value>, field: &str, type_name: &str) -> Result<Vec<String>, AppError> {
    let Some(value) = obj.get(field) else {
        return Err(invalid(format!("type={type_name}: data.{field} is required")));
    };
    let Some(items) = value.as_array() else {
        return Err(invalid(format!("type={type_name}: data.{field} must be an array")));
    };
    items
        .iter()
        .map(|item| item.as_str().map(str::to_string).ok_or_else(|| invalid(format!("type={type_name}: every entry of data.{field} must be a string"))))
        .collect()
}

fn require_non_empty_string_array(obj: &serde_json::Map<String, Value>, field: &str, type_name: &str) -> Result<(), AppError> {
    let items = require_string_array(obj, field, type_name)?;
    if items.is_empty() {
        return Err(invalid(format!("type={type_name}: data.{field} must not be empty")));
    }
    Ok(())
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
            validate_media_source(obj, "audio")?;
            validate_media_display(obj, "audio")?;
            reject_unknown_fields(obj, &["asset", "src", "title", "width"], "audio")
        }
        "video" => {
            let obj = as_object(data, "video")?;
            validate_media_source(obj, "video")?;
            validate_media_display(obj, "video")?;
            reject_unknown_fields(obj, &["asset", "src", "title", "width"], "video")
        }
        "image" => {
            let obj = as_object(data, "image")?;
            validate_media_source(obj, "image")?;
            validate_media_display(obj, "image")?;
            reject_unknown_fields(obj, &["asset", "src", "alt", "caption", "width"], "image")
        }
        // Any embeddable page — a chart, a map, a simulation, a form.
        "embed" => {
            let obj = as_object(data, "embed")?;
            validate_media_source(obj, "embed")?;
            validate_media_display(obj, "embed")?;
            if let Some(height) = obj.get("height") {
                // ALM is a text format, so a height written by hand
                // arrives as "480" while one set in the editor arrives as
                // 480 — accept both rather than making the two paths
                // disagree about the same document.
                let parsed = height.as_i64().or_else(|| height.as_str().and_then(|s| s.trim().parse::<i64>().ok()));
                match parsed {
                    Some(h) if (120..=2000).contains(&h) => {}
                    _ => return Err(invalid("type=embed: data.height must be 120-2000 (pixels)".to_string())),
                }
            }
            reject_unknown_fields(obj, &["asset", "src", "title", "width", "height"], "embed")
        }
        // A file the learner downloads rather than plays inline.
        "file" => {
            let obj = as_object(data, "file")?;
            validate_media_source(obj, "file")?;
            reject_unknown_fields(obj, &["asset", "src", "label", "description"], "file")
        }
        "divider" => {
            let obj = as_object(data, "divider")?;
            reject_unknown_fields(obj, &[], "divider")
        }
        // Collapsible section — Notion's toggle.
        "toggle" => {
            let obj = as_object(data, "toggle")?;
            require_non_empty_string(obj, "title", "toggle")?;
            require_non_empty_string(obj, "content", "toggle")?;
            reject_unknown_fields(obj, &["title", "content"], "toggle")
        }
        // Side-by-side prose.
        "columns" => {
            let obj = as_object(data, "columns")?;
            require_non_empty_string(obj, "left", "columns")?;
            require_non_empty_string(obj, "right", "columns")?;
            reject_unknown_fields(obj, &["left", "right"], "columns")
        }
        // A link rendered as a card rather than inline text.
        "bookmark" => {
            let obj = as_object(data, "bookmark")?;
            let url = obj.get("url").and_then(|v| v.as_str()).unwrap_or_default();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(invalid("type=bookmark: data.url must be an http(s) url".to_string()));
            }
            reject_unknown_fields(obj, &["url", "title", "description"], "bookmark")
        }
        // ── Subject-general blocks ──
        // The set below is deliberately not tied to language learning:
        // a table, a formula, a procedure and a timeline are what
        // Matematika, Fisika, Sejarah and Informatika modules are made
        // of, and none of them had a home before.
        "list" => {
            let obj = as_object(data, "list")?;
            require_non_empty_string_array(obj, "items", "list")?;
            if let Some(ordered) = obj.get("ordered") {
                if !ordered.is_boolean() {
                    return Err(invalid("type=list: data.ordered must be a boolean".to_string()));
                }
            }
            reject_unknown_fields(obj, &["items", "ordered"], "list")
        }
        "table" => {
            let obj = as_object(data, "table")?;
            let headers = require_string_array(obj, "headers", "table")?;
            let Some(rows) = obj.get("rows").and_then(|v| v.as_array()) else {
                return Err(invalid("type=table: data.rows must be an array".to_string()));
            };
            for (index, row) in rows.iter().enumerate() {
                let Some(cells) = row.as_array() else {
                    return Err(invalid(format!("type=table: data.rows[{index}] must be an array of cells")));
                };
                if !cells.iter().all(|c| c.is_string()) {
                    return Err(invalid(format!("type=table: every cell of data.rows[{index}] must be a string")));
                }
                // A ragged table renders with holes or drops data
                // silently, so catch it at authoring time instead.
                if !headers.is_empty() && cells.len() != headers.len() {
                    return Err(invalid(format!(
                        "type=table: data.rows[{index}] has {} cells but there are {} headers",
                        cells.len(),
                        headers.len()
                    )));
                }
            }
            reject_unknown_fields(obj, &["headers", "rows", "caption"], "table")
        }
        "code" => {
            let obj = as_object(data, "code")?;
            require_non_empty_string(obj, "code", "code")?;
            reject_unknown_fields(obj, &["code", "language", "caption"], "code")
        }
        "formula" => {
            let obj = as_object(data, "formula")?;
            require_non_empty_string(obj, "latex", "formula")?;
            reject_unknown_fields(obj, &["latex", "caption"], "formula")
        }
        "steps" => {
            let obj = as_object(data, "steps")?;
            require_non_empty_string_array(obj, "items", "steps")?;
            reject_unknown_fields(obj, &["items", "title"], "steps")
        }
        "definition" => {
            let obj = as_object(data, "definition")?;
            require_non_empty_string(obj, "term", "definition")?;
            require_non_empty_string(obj, "definition", "definition")?;
            reject_unknown_fields(obj, &["term", "definition"], "definition")
        }
        "callout" => {
            let obj = as_object(data, "callout")?;
            require_non_empty_string(obj, "text", "callout")?;
            // One generic callout with a variant, rather than a new
            // block type per tone — `indonesian_learner_alert` was the
            // English-only ancestor of exactly this.
            if let Some(variant) = obj.get("variant") {
                let allowed = ["info", "tip", "warning", "important"];
                match variant.as_str() {
                    Some(v) if allowed.contains(&v) => {}
                    _ => {
                        return Err(invalid(format!("type=callout: data.variant must be one of [{}]", allowed.join(", "))));
                    }
                }
            }
            reject_unknown_fields(obj, &["text", "variant", "title"], "callout")
        }
        "timeline" => {
            let obj = as_object(data, "timeline")?;
            let Some(entries) = obj.get("entries").and_then(|v| v.as_array()) else {
                return Err(invalid("type=timeline: data.entries must be an array".to_string()));
            };
            if entries.is_empty() {
                return Err(invalid("type=timeline: data.entries must not be empty".to_string()));
            }
            for (index, entry) in entries.iter().enumerate() {
                let Some(entry_obj) = entry.as_object() else {
                    return Err(invalid(format!("type=timeline: data.entries[{index}] must be an object")));
                };
                require_non_empty_string(entry_obj, "label", "timeline")?;
                require_non_empty_string(entry_obj, "text", "timeline")?;
                reject_unknown_fields(entry_obj, &["label", "text"], "timeline")?;
            }
            reject_unknown_fields(obj, &["entries", "title"], "timeline")
        }
        "comparison" => {
            let obj = as_object(data, "comparison")?;
            require_non_empty_string(obj, "left_label", "comparison")?;
            require_non_empty_string(obj, "right_label", "comparison")?;
            let Some(rows) = obj.get("rows").and_then(|v| v.as_array()) else {
                return Err(invalid("type=comparison: data.rows must be an array".to_string()));
            };
            for (index, row) in rows.iter().enumerate() {
                let ok = row.as_array().is_some_and(|pair| pair.len() == 2 && pair.iter().all(|c| c.is_string()));
                if !ok {
                    return Err(invalid(format!("type=comparison: data.rows[{index}] must be a pair of strings")));
                }
            }
            reject_unknown_fields(obj, &["left_label", "right_label", "rows"], "comparison")
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
