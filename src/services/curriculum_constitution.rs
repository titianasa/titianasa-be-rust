use crate::errors::AppError;

// Port of curriculum_constitution.ts. P2-011 / ALR_Phase_Detail_Breakdown.md
// 2.9 — the grammar lesson Constitution. agent/docs/curriculum-constitution.md
// is the human-reviewed spec this mirrors; the two must stay in sync.

// [prefix, section title] in required order. A heading block satisfies a
// section when its text starts with that section's prefix. Public so
// ai_content.rs's lesson-generation prompt can spell out the same 11
// sections to the model, not just validate them after the fact.
pub const SECTIONS: [(&str, &str); 11] = [
    ("01", "What is it?"),
    ("02", "Form"),
    ("03", "Positive"),
    ("04", "Negative"),
    ("05", "Question"),
    ("06", "When to use it?"),
    ("07", "Signal words"),
    ("08", "Common mistakes"),
    ("09", "Practice"),
    ("10", "Speaking"),
    ("11", "Writing"),
];

pub struct TypedBlock {
    pub r#type: String,
    pub data: serde_json::Value,
}

// Checked at POST /lessons/{id}/submit-review for any lesson linked to a
// concepts.type = 'grammar' concept — a draft-in-progress is never
// blocked by this, only the move toward in_review/published.
pub fn validate_grammar_lesson(blocks: &[TypedBlock]) -> Result<(), AppError> {
    let headings: Vec<String> = blocks
        .iter()
        .filter(|b| b.r#type == "heading")
        .filter_map(|b| b.data.get("text").and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string())
        .collect();

    let missing: Vec<String> = SECTIONS
        .iter()
        .filter(|(prefix, _)| !headings.iter().any(|h| h.starts_with(prefix)))
        .map(|(prefix, title)| format!("{prefix} — {title}"))
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    Err(AppError::UnprocessableEntity(
        "grammar_constitution_incomplete",
        format!("missing required section(s): {}", missing.join("; ")),
    ))
}
