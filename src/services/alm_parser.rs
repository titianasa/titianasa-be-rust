use std::collections::BTreeMap;

use crate::errors::AppError;

// Port of alm_parser.ts. P2-006: ALR Learning Markdown (ALM) parser.
// Markdown extended with `:::type ... :::` directive blocks. Produces
// the Semantic AST as a flat list of typed blocks; the actual *validity*
// of each block's shape is block_schema's job — this parser only
// extracts structure, never hardcodes which directive types are legal.
//
// Question embedding is by-reference only: the only supported embed
// syntax is `:::question_embed\nquestion_id: <uuid>\n:::`.

#[derive(Debug, Clone)]
pub struct ParsedBlock {
    pub r#type: String,
    pub data: serde_json::Value,
    pub raw_source: String,
}

fn invalid(detail: String) -> AppError {
    AppError::UnprocessableEntity("invalid_alm_source", detail)
}

pub fn parse(source: &str) -> Result<Vec<ParsedBlock>, AppError> {
    let lines: Vec<&str> = source.split(['\n']).map(|l| l.trim_end_matches('\r')).collect();
    let mut blocks = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];

        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        let trimmed_start = line.trim_start();
        if let Some(directive_type) = trimmed_start.strip_prefix(":::") {
            let directive_type = directive_type.trim();
            if directive_type.is_empty() {
                return Err(invalid(format!(r#"line {}: ":::" must be followed by a block type"#, i + 1)));
            }
            let directive_type = directive_type.to_string();
            let start = i;
            let body_start = i + 1;
            let mut body_end: Option<usize> = None;
            let mut j = body_start;
            while j < lines.len() {
                if lines[j].trim() == ":::" {
                    body_end = Some(j);
                    break;
                }
                j += 1;
            }
            let Some(body_end) = body_end else {
                return Err(invalid(format!(
                    r#"line {}: ":::{directive_type}" was never closed with a matching ":::""#,
                    start + 1
                )));
            };
            let body_lines = &lines[body_start..body_end];
            let raw_source = lines[start..=body_end].join("\n");

            let data = if directive_type == "example" {
                serde_json::json!({ "text": body_lines.join("\n").trim() })
            } else {
                parse_key_value_body(body_lines)
            };

            blocks.push(ParsedBlock { r#type: directive_type, data, raw_source });
            i = body_end + 1;
            continue;
        }

        if let Some((text, level)) = heading_text(line) {
            blocks.push(ParsedBlock {
                r#type: "heading".to_string(),
                data: serde_json::json!({ "text": text, "level": level }),
                raw_source: line.to_string(),
            });
            i += 1;
            continue;
        }

        if line.trim_start().starts_with('>') {
            let start = i;
            let mut quote_lines = Vec::new();
            while i < lines.len() {
                let trimmed = lines[i].trim_start();
                if let Some(rest) = trimmed.strip_prefix('>') {
                    quote_lines.push(rest.trim().to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(ParsedBlock {
                r#type: "example".to_string(),
                data: serde_json::json!({ "text": quote_lines.join(" ") }),
                raw_source: lines[start..i].join("\n"),
            });
            continue;
        }

        if let Some((alt, asset)) = image_syntax(line) {
            blocks.push(ParsedBlock {
                r#type: "image".to_string(),
                data: serde_json::json!({ "asset": asset, "alt": alt }),
                raw_source: line.to_string(),
            });
            i += 1;
            continue;
        }

        // Plain paragraph — accumulate consecutive non-special lines
        // into one text block.
        let start = i;
        let mut paragraph_lines = Vec::new();
        while i < lines.len() {
            let l = lines[i];
            if l.trim().is_empty()
                || l.trim_start().starts_with(":::")
                || l.trim_start().starts_with('>')
                || heading_text(l).is_some()
                || image_syntax(l).is_some()
            {
                break;
            }
            paragraph_lines.push(l);
            i += 1;
        }
        blocks.push(ParsedBlock {
            r#type: "text".to_string(),
            data: serde_json::json!({ "text": paragraph_lines.join(" ").trim() }),
            raw_source: lines[start..i].join("\n"),
        });
    }

    Ok(blocks)
}

fn count_leading_hashes(line: &str) -> usize {
    line.trim_start().chars().take_while(|&c| c == '#').count()
}

fn heading_text(line: &str) -> Option<(String, u32)> {
    let trimmed = line.trim_start();
    let hashes = count_leading_hashes(trimmed);
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    let rest = rest.strip_prefix(' ')?;
    Some((rest.trim().to_string(), hashes as u32))
}

// `![alt text](asset://...)` — only `asset://` references are accepted
// (media is always by-reference, never a raw URL).
fn image_syntax(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('!')?;
    let rest = rest.strip_prefix('[')?;
    let close_bracket = rest.find(']')?;
    let alt = rest[..close_bracket].to_string();
    let rest = &rest[close_bracket + 1..];
    let rest = rest.strip_prefix('(')?;
    let rest = rest.strip_suffix(')')?;
    Some((alt, rest.to_string()))
}

fn parse_key_value_body(lines: &[&str]) -> serde_json::Value {
    let mut map = BTreeMap::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Some(colon_index) = line.find(':') else { continue };
        let key = line[..colon_index].trim().to_string();
        let mut value = line[colon_index + 1..].trim().to_string();
        if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
            value = value[1..value.len() - 1].to_string();
        }
        map.insert(key, serde_json::Value::String(value));
    }
    serde_json::to_value(map).unwrap()
}
