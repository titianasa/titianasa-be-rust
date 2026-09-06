// Port of paste_normalizer.ts. P2-007: Paste Normalizer. Markdown needs
// no work (it already *is* ALM syntax for the tags ALM understands);
// HTML gets converted to the equivalent ALM text first. A small
// block-level tag scanner, not a full HTML5 parser — deliberately
// scoped to what paste-from-Word/Google-Docs/website actually produces.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Markdown,
    Html,
}

pub fn normalize(source: &str, format: SourceFormat) -> String {
    match format {
        SourceFormat::Markdown => source.to_string(),
        SourceFormat::Html => normalize_html(source),
    }
}

fn normalize_html(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut blocks: Vec<String> = Vec::new();
    let mut pos = 0usize;

    loop {
        let Some(open_start) = find_from(&chars, pos, '<') else { break };
        let Some(open_end) = find_from(&chars, open_start, '>') else { break };
        let tag_content: String = chars[open_start + 1..open_end].iter().collect();

        if tag_content.starts_with('/') || tag_content.starts_with('!') {
            pos = open_end + 1;
            continue;
        }

        let tag_name: String = tag_content
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();

        if tag_name == "img" {
            if let Some(src) = extract_attr(&tag_content, "src") {
                let alt = extract_attr(&tag_content, "alt").unwrap_or_default();
                blocks.push(format!("![{alt}]({src})"));
            }
            pos = open_end + 1;
            continue;
        }

        let close_tag = format!("</{tag_name}>");
        let search_area: String = chars[(open_end + 1).min(len)..].iter().collect();
        let rel_close = search_area.to_lowercase().find(&close_tag);

        if let Some(rel_close) = rel_close {
            let close_start = open_end + 1 + rel_close;
            let inner_raw: String = chars[(open_end + 1).min(len)..close_start.min(len)].iter().collect();
            let inner = strip_tags(inner_raw.trim());
            let close_end = close_start + close_tag.chars().count();

            match tag_name.as_str() {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level: usize = tag_name[1..].parse().unwrap_or(1);
                    blocks.push(format!("{} {inner}", "#".repeat(level)));
                }
                "p" => {
                    if !inner.is_empty() {
                        blocks.push(inner);
                    }
                }
                "blockquote" => blocks.push(format!("> {inner}")),
                _ => {
                    // Unrecognized block tag: skip the wrapper, keep
                    // scanning — inner content with recognized tags
                    // still gets picked up on later iterations since
                    // `pos` only advances past the opening tag here.
                    pos = open_end + 1;
                    continue;
                }
            }
            pos = close_end;
        } else {
            // No matching close tag — skip past the opening tag rather
            // than looping forever on malformed input.
            pos = open_end + 1;
        }
    }

    blocks.join("\n\n")
}

fn find_from(chars: &[char], from: usize, needle: char) -> Option<usize> {
    chars[from.min(chars.len())..].iter().position(|&c| c == needle).map(|i| i + from)
}

fn strip_tags(text: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_attr(tag_content: &str, attr: &str) -> Option<String> {
    let needle = format!(r#"{attr}=""#);
    let start = tag_content.find(&needle)? + needle.len();
    let rest = &tag_content[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
