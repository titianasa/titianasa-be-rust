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

        // Fenced code — ```lang ... ```. Informatika lessons need real
        // code, and TipTap's native codeBlock node round-trips to this.
        if let Some(language) = code_fence_open(line) {
            let start = i;
            let body_start = i + 1;
            let mut body_end = lines.len();
            let mut j = body_start;
            while j < lines.len() {
                if lines[j].trim_start().starts_with("```") {
                    body_end = j;
                    break;
                }
                j += 1;
            }
            let code = lines[body_start..body_end.min(lines.len())].join("\n");
            blocks.push(ParsedBlock {
                r#type: "code".to_string(),
                data: serde_json::json!({ "language": language, "code": code }),
                raw_source: lines[start..(body_end + 1).min(lines.len())].join("\n"),
            });
            i = (body_end + 1).min(lines.len());
            continue;
        }

        // Bullet / numbered lists. Consecutive marker lines fold into one
        // block; before this they fell through to the paragraph branch and
        // were silently flattened into prose.
        if let Some((ordered, _)) = list_item(line) {
            let start = i;
            let mut items = Vec::new();
            while i < lines.len() {
                match list_item(lines[i]) {
                    Some((is_ordered, text)) if is_ordered == ordered => {
                        items.push(serde_json::Value::String(text));
                        i += 1;
                    }
                    _ => break,
                }
            }
            blocks.push(ParsedBlock {
                r#type: "list".to_string(),
                data: serde_json::json!({ "ordered": ordered, "items": items }),
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
                || code_fence_open(l).is_some()
                || list_item(l).is_some()
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

// ```` ```python ```` opens a fenced code block; the info string (may be
// empty) is the language.
fn code_fence_open(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("```")?;
    Some(rest.trim().to_string())
}

// `- item` / `* item` (unordered) or `1. item` (ordered). Returns
// (ordered, text).
fn list_item(line: &str) -> Option<(bool, String)> {
    let trimmed = line.trim_start();
    for marker in ["- ", "* "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some((false, rest.trim().to_string()));
        }
    }
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = trimmed[digits.len()..].strip_prefix(". ")?;
    Some((true, rest.trim().to_string()))
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
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty() {
            continue;
        }
        let Some(colon_index) = line.find(':') else { continue };
        let key = line[..colon_index].trim().to_string();
        let mut value = line[colon_index + 1..].trim().to_string();
        // A JSON value the writer spread over several lines. The guide
        // asks for one line per key, but models pretty-print arrays
        // anyway, and a `rows: [` read on its own becomes the string
        // "[" — the block then fails its schema and is demoted, which
        // is how raw JSON ends up rendered as prose to a learner.
        if unbalanced(&value) {
            while i < lines.len() && unbalanced(&value) {
                value.push(' ');
                value.push_str(lines[i].trim());
                i += 1;
            }
        }
        map.insert(key, parse_scalar(&value));
    }
    serde_json::to_value(map).unwrap()
}

/// True while `value` has an open bracket or brace outside a string.
/// Only opened values are continued, so prose with a stray "]" is
/// untouched.
fn unbalanced(value: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut opened = false;
    for ch in value.chars() {
        if in_string {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' | '{' => {
                depth += 1;
                opened = true;
            }
            ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    opened && (depth > 0 || in_string)
}

// A directive field is usually a plain string, but the structured blocks
// (a table's rows, a timeline's entries, a list's items) need real
// arrays. Anything that parses as a JSON array or object is stored as
// one; everything else stays a string, so ordinary prose containing a
// stray bracket is never mangled into JSON.
fn parse_scalar(value: &str) -> serde_json::Value {
    if (value.starts_with('[') && value.ends_with(']')) || (value.starts_with('{') && value.ends_with('}')) {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) {
            return parsed;
        }
    }
    let unquoted = if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') { &value[1..value.len() - 1] } else { value };
    serde_json::Value::String(unquoted.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn types(source: &str) -> Vec<String> {
        parse(source).unwrap().iter().map(|b| b.r#type.clone()).collect()
    }

    #[test]
    fn json_value_spread_over_several_lines_is_one_value() {
        // Models pretty-print `rows` no matter what the guide says.
        // Read line by line, `rows` became the string "[" — the table
        // then failed its schema and was demoted, so the learner saw
        // the raw JSON as prose.
        let blocks = parse(":::table\nheaders: [\"A\", \"B\"]\nrows: [\n  [\"1\", \"2\"],\n  [\"3\", \"4\"]\n]\n:::").unwrap();
        assert_eq!(blocks[0].data["rows"], json!([["1", "2"], ["3", "4"]]));
    }

    #[test]
    fn a_bracket_inside_prose_does_not_swallow_the_next_lines() {
        let blocks = parse(":::callout\ntext: Tulis [di sini] lalu berhenti.\nvariant: tip\n:::").unwrap();
        assert_eq!(blocks[0].data["text"], json!("Tulis [di sini] lalu berhenti."));
        assert_eq!(blocks[0].data["variant"], json!("tip"));
    }

    #[test]
    fn a_bracket_inside_a_json_string_does_not_end_the_value() {
        let blocks = parse(":::steps\nitems: [\n  \"Kurung ] di dalam teks\",\n  \"Langkah dua\"\n]\n:::").unwrap();
        assert_eq!(blocks[0].data["items"], json!(["Kurung ] di dalam teks", "Langkah dua"]));
    }

    #[test]
    fn bullet_and_numbered_lists_become_list_blocks() {
        // Before this they fell through to the paragraph branch and were
        // flattened into one line of prose, losing the list entirely.
        let blocks = parse("- satu\n- dua\n\n1. pertama\n2. kedua").unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].r#type, "list");
        assert_eq!(blocks[0].data, json!({"ordered": false, "items": ["satu", "dua"]}));
        assert_eq!(blocks[1].data, json!({"ordered": true, "items": ["pertama", "kedua"]}));
    }

    #[test]
    fn an_ordered_and_unordered_run_do_not_merge() {
        let blocks = parse("- satu\n1. dua").unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].data["ordered"], json!(false));
        assert_eq!(blocks[1].data["ordered"], json!(true));
    }

    #[test]
    fn fenced_code_keeps_its_language_and_indentation() {
        let blocks = parse("```python\ndef f():\n    return 1\n```").unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].r#type, "code");
        assert_eq!(blocks[0].data["language"], json!("python"));
        assert_eq!(blocks[0].data["code"], json!("def f():\n    return 1"));
    }

    #[test]
    fn directive_values_that_are_json_parse_as_json() {
        let blocks = parse(":::table\nheaders: [\"Unsur\", \"Simbol\"]\nrows: [[\"Hidrogen\", \"H\"]]\ncaption: Tabel unsur\n:::").unwrap();
        assert_eq!(blocks[0].data["headers"], json!(["Unsur", "Simbol"]));
        assert_eq!(blocks[0].data["rows"], json!([["Hidrogen", "H"]]));
        assert_eq!(blocks[0].data["caption"], json!("Tabel unsur"));
    }

    #[test]
    fn prose_containing_a_bracket_is_not_mangled_into_json() {
        let blocks = parse(":::callout\ntext: Perhatikan [catatan kaki] di bawah\n:::").unwrap();
        assert_eq!(blocks[0].data["text"], json!("Perhatikan [catatan kaki] di bawah"));
    }

    #[test]
    fn a_hyphen_in_prose_does_not_start_a_list() {
        // "-5 derajat" has no space after the hyphen, so it stays prose.
        assert_eq!(types("-5 derajat celsius"), vec!["text"]);
    }

    #[test]
    fn existing_syntax_still_parses_unchanged() {
        assert_eq!(
            types("# Judul\nParagraf\n> kutipan\n![a](asset://x)\n:::flashcard\nfront: a\nback: b\n:::"),
            vec!["heading", "text", "example", "image", "flashcard"]
        );
    }
}
