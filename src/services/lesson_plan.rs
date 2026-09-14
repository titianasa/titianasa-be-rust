// An `article` item as a "Modul Belajar": an ordered list of sections
// (title, minutes, goal, ALM body), ported from parelabs'
// module_item.config.lesson_plan.
//
// One source serves every surface — the author edits sections, the
// learner reads them section by section — and the plan is ALSO
// projected into content_blocks on every save, so QA and anything else
// that reads blocks keeps working unchanged.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::{alm_parser, block_schema};

pub const MAX_SECTIONS: usize = 40;

/// Blocks a model must never emit: they point at rows by id, and an id
/// the model invented would fail the save outright.
const NOT_GENERATABLE: [&str; 2] = ["question_embed", "assessment_embed"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LessonPlan {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub level: String,
    /// The language the plan is WRITTEN in. The learner may read it in
    /// another one (see module_item_translations).
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub sections: Vec<LessonPlanSection>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LessonPlanSection {
    /// Stable across reorders, so an editor can key its per-section
    /// state by it rather than by position.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<i64>,
    #[serde(default)]
    pub goal: String,
    /// ALM source.
    #[serde(default)]
    pub content: String,
    /// The comprehension check at the end of this section: a small pool
    /// of questions in the quiz shape (`{"question_groups": [...]}`, flat
    /// auto-graded subtypes only) that `section_checkpoint` draws from.
    /// Never sent to a learner (`learner_view`) and never projected into
    /// content_blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<serde_json::Value>,
}

/// Default number of questions a checkpoint draws from its pool.
pub const CHECKPOINT_DRAW: i64 = 2;

fn invalid(detail: String) -> AppError {
    AppError::UnprocessableEntity("invalid_lesson_plan", detail)
}

pub fn parse(value: &serde_json::Value) -> Result<LessonPlan, AppError> {
    serde_json::from_value(value.clone()).map_err(|e| invalid(format!("lesson_plan tidak sesuai bentuknya: {e}")))
}

pub fn new_section_id() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Points an error at the section it came from — "Bagian 3: ..." is
/// actionable, a bare block error in a 10-section module is not.
fn in_section(index: usize, err: AppError) -> AppError {
    match err {
        AppError::UnprocessableEntity(code, detail) => AppError::UnprocessableEntity(code, format!("Bagian {}: {detail}", index + 1)),
        other => other,
    }
}

pub fn validate_content(content: &str) -> Result<(), AppError> {
    for block in alm_parser::parse(content)? {
        block_schema::validate(&block.r#type, &block.data)?;
    }
    Ok(())
}

/// Trims, fills missing/duplicate section ids, clamps minutes, and
/// validates every section's ALM.
pub fn normalize(mut plan: LessonPlan) -> Result<LessonPlan, AppError> {
    if plan.sections.len() > MAX_SECTIONS {
        return Err(invalid(format!("paling banyak {MAX_SECTIONS} bagian")));
    }
    plan.title = one_line(&plan.title);
    plan.topic = plan.topic.trim().to_string();
    plan.level = one_line(&plan.level);
    plan.language = plan.language.trim().to_lowercase();

    let mut seen = HashSet::new();
    for (index, section) in plan.sections.iter_mut().enumerate() {
        let id = section.id.trim().to_string();
        section.id = if id.is_empty() || seen.contains(&id) { new_section_id() } else { id };
        seen.insert(section.id.clone());
        section.title = one_line(&section.title);
        section.goal = section.goal.trim().to_string();
        section.minutes = section.minutes.map(|m| m.clamp(1, 600));
        section.content = section.content.trim().to_string();
        validate_content(&section.content).map_err(|e| in_section(index, e))?;
        if let Some(raw) = section.checkpoint.take() {
            section.checkpoint = normalize_checkpoint(&raw).map_err(|e| in_section(index, e))?;
        }
    }
    Ok(plan)
}

/// Parses a checkpoint pool, gives its questions permanent uids and order
/// flags, and rejects what a checkpoint can't grade instantly: anything
/// but a flat, auto-scored subtype. An empty pool normalizes to None.
fn normalize_checkpoint(raw: &serde_json::Value) -> Result<Option<serde_json::Value>, AppError> {
    use crate::services::{quiz_config, quiz_config_schema, quiz_subtype};
    let mut pool = quiz_config_schema::parse(raw).map_err(|_| invalid("checkpoint harus berbentuk {\"question_groups\": [...]}".to_string()))?;
    pool.question_groups.retain(|g| !g.questions.is_empty());
    if pool.question_groups.is_empty() {
        return Ok(None);
    }
    for group in &pool.question_groups {
        let ok = quiz_subtype::find(&group.r#type)
            .is_some_and(|info| matches!(info.grading_mode, quiz_subtype::GradingMode::Auto) && matches!(info.layout, quiz_subtype::LayoutKind::Flat));
        if !ok {
            return Err(invalid(format!("checkpoint tidak bisa memakai jenis soal \"{}\" — hanya jenis yang dinilai otomatis satu per soal", group.r#type)));
        }
    }
    quiz_config::ensure_question_uids(&mut pool);
    quiz_config::detect_order_constraints(&mut pool);
    Ok(Some(serde_json::to_value(&pool).map_err(|e| AppError::Internal(e.into()))?))
}

/// The plan as a learner may receive it: each section's checkpoint pool
/// carries answer keys, and a learner is handed checkpoint questions one
/// server-side draw at a time (`section_checkpoint`), never the pool.
pub fn learner_view(raw: &serde_json::Value) -> serde_json::Value {
    let mut out = raw.clone();
    if let Some(sections) = out.get_mut("sections").and_then(serde_json::Value::as_array_mut) {
        for section in sections.iter_mut().filter_map(serde_json::Value::as_object_mut) {
            section.remove("checkpoint");
        }
    }
    out
}

/// The plan as one ALM document, for content_blocks.
pub fn to_alm(plan: &LessonPlan) -> String {
    plan.sections
        .iter()
        .enumerate()
        .map(|(index, section)| {
            let title = if section.title.is_empty() { format!("Bagian {}", index + 1) } else { section.title.clone() };
            if section.content.is_empty() {
                format!("## {title}")
            } else {
                format!("## {title}\n\n{}", section.content)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn flatten_json(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
        serde_json::Value::Array(items) => items.iter().for_each(|v| flatten_json(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| flatten_json(v, out)),
        _ => {}
    }
}

/// Key names a model reaches for that the schema does not have. A
/// rename keeps the block: the content is right, only the label is
/// wrong, and demoting it would throw away a whole table to punish one
/// word.
const KEY_ALIASES: &[(&str, &str, &str)] = &[
    ("table", "title", "caption"),
    ("steps", "steps", "items"),
    ("steps", "langkah", "items"),
    ("callout", "body", "text"),
    ("callout", "content", "text"),
    ("callout", "isi", "text"),
    ("definition", "name", "term"),
    ("definition", "meaning", "definition"),
    ("comparison", "left", "left_label"),
    ("comparison", "right", "right_label"),
    ("timeline", "items", "entries"),
    ("toggle", "text", "content"),
    ("formula", "formula", "latex"),
    ("code", "source", "code"),
];

/// The block with its aliased keys renamed, if that makes it valid.
fn repair_block(block: &alm_parser::ParsedBlock) -> Option<String> {
    if NOT_GENERATABLE.contains(&block.r#type.as_str()) {
        return None;
    }
    let obj = block.data.as_object()?;
    let mut fixed = obj.clone();
    let mut changed = false;
    for (kind, from, to) in KEY_ALIASES {
        if *kind == block.r#type && !fixed.contains_key(*to) {
            if let Some(value) = fixed.remove(*from) {
                fixed.insert((*to).to_string(), value);
                changed = true;
            }
        }
    }
    if !changed {
        return None;
    }
    let data = serde_json::Value::Object(fixed);
    block_schema::validate(&block.r#type, &data).ok()?;
    block_to_alm(&block.r#type, &data)
}

/// A block written back as ALM — one line per key, JSON values inline,
/// which is the only form the parser reads back.
fn block_to_alm(kind: &str, data: &serde_json::Value) -> Option<String> {
    let obj = data.as_object()?;
    let mut out = format!(":::{kind}\n");
    for (key, value) in obj {
        let rendered = match value {
            serde_json::Value::String(text) => text.clone(),
            other => serde_json::to_string(other).ok()?,
        };
        if rendered.contains('\n') || key.contains(':') {
            return None;
        }
        out.push_str(&format!("{key}: {rendered}\n"));
    }
    out.push_str(":::");
    Some(out)
}

/// A directive block the schema rejected, turned back into prose.
/// Losing the formatting is far better than losing the words — but the
/// words have to stay readable: a table's `rows` demoted to its own
/// JSON source is what a learner then sees on the page, brackets and
/// quotes and all. So arrays become lists, rows become one bullet per
/// row, and nothing is ever emitted as JSON.
fn demote_directive(block: &alm_parser::ParsedBlock) -> String {
    let Some(obj) = block.data.as_object() else {
        return demote_raw(&block.raw_source);
    };
    if obj.is_empty() {
        return demote_raw(&block.raw_source);
    }
    let mut out: Vec<String> = Vec::new();
    for value in obj.values() {
        match value {
            serde_json::Value::String(text) if !text.trim().is_empty() => out.push(text.trim().to_string()),
            serde_json::Value::Array(items) if !items.is_empty() => {
                let bullets: Vec<String> = items
                    .iter()
                    .map(|item| {
                        let mut parts = Vec::new();
                        flatten_json(item, &mut parts);
                        parts.join(" · ")
                    })
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| format!("- {line}"))
                    .collect();
                if !bullets.is_empty() {
                    out.push(bullets.join("\n"));
                }
            }
            serde_json::Value::Object(_) => {
                let mut parts = Vec::new();
                flatten_json(value, &mut parts);
                if !parts.is_empty() {
                    out.push(parts.join(" · "));
                }
            }
            _ => {}
        }
    }
    if out.is_empty() { demote_raw(&block.raw_source) } else { out.join("\n\n") }
}

/// Last resort for a block whose body is not `key: value` at all.
fn demote_raw(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(":::"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Makes model-written ALM always saveable. Valid content passes
/// through untouched; each invalid block is demoted to prose; content
/// the parser cannot read at all (an unterminated directive or code
/// fence) loses its fence lines instead.
pub fn sanitize_generated(content: &str) -> String {
    let content = content.trim();
    let blocks = match alm_parser::parse(content) {
        Ok(blocks) => blocks,
        Err(_) => {
            let without_directives: String = content.lines().filter(|l| !l.trim_start().starts_with(":::")).collect::<Vec<_>>().join("\n");
            if alm_parser::parse(&without_directives).is_ok() {
                return without_directives.trim().to_string();
            }
            return without_directives.lines().filter(|l| !l.trim_start().starts_with("```")).collect::<Vec<_>>().join("\n").trim().to_string();
        }
    };

    let is_ok = |b: &alm_parser::ParsedBlock| !NOT_GENERATABLE.contains(&b.r#type.as_str()) && block_schema::validate(&b.r#type, &b.data).is_ok();
    if blocks.iter().all(is_ok) {
        return content.to_string();
    }
    blocks
        .iter()
        .map(|b| if is_ok(b) { b.raw_source.clone() } else { repair_block(b).unwrap_or_else(|| demote_directive(b)) })
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub async fn load(pool: &PgPool, item_id: Uuid) -> Result<Option<LessonPlan>, AppError> {
    let raw = sqlx::query_scalar!(r#"select lesson_plan from module_items where id = $1"#, item_id)
        .fetch_optional(pool)
        .await?
        .flatten();
    raw.map(|v| parse(&v)).transpose()
}

/// FNV-1a over the serialized plan — stable across processes and
/// builds, unlike std's DefaultHasher, which matters for a value that
/// is persisted.
pub fn source_hash(plan: &LessonPlan) -> String {
    let text = serde_json::to_string(plan).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub async fn cached_translation(pool: &PgPool, item_id: Uuid, language: &str, hash: &str) -> Result<Option<LessonPlan>, AppError> {
    let raw = sqlx::query_scalar!(
        r#"select lesson_plan from module_item_translations where item_id = $1 and language = $2 and source_hash = $3"#,
        item_id,
        language,
        hash,
    )
    .fetch_optional(pool)
    .await?;
    Ok(raw.and_then(|v| serde_json::from_value(v).ok()))
}

pub async fn store_translation(pool: &PgPool, item_id: Uuid, language: &str, hash: &str, plan: &LessonPlan) -> Result<(), AppError> {
    let value = serde_json::to_value(plan).map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query!(
        r#"insert into module_item_translations (item_id, language, source_hash, lesson_plan)
           values ($1, $2, $3, $4)
           on conflict (item_id, language)
           do update set source_hash = excluded.source_hash, lesson_plan = excluded.lesson_plan, created_at = now()"#,
        item_id,
        language,
        hash,
        value,
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(id: &str, content: &str) -> LessonPlanSection {
        LessonPlanSection { id: id.to_string(), title: "Judul".to_string(), minutes: Some(5), goal: String::new(), content: content.to_string(), checkpoint: None }
    }

    #[test]
    fn normalize_fills_missing_and_duplicate_ids() {
        let plan = LessonPlan { sections: vec![section("", "Teks."), section("a", "Teks."), section("a", "Teks.")], ..Default::default() };
        let out = normalize(plan).unwrap();
        let ids: HashSet<_> = out.sections.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(out.sections[1].id, "a");
    }

    #[test]
    fn normalize_names_the_failing_section() {
        let plan = LessonPlan { sections: vec![section("a", "Teks."), section("b", ":::callout\nvariant: loud\ntext: x\n:::")], ..Default::default() };
        let err = normalize(plan).unwrap_err();
        match err {
            AppError::UnprocessableEntity(_, detail) => assert!(detail.starts_with("Bagian 2:"), "{detail}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn to_alm_heads_every_section() {
        let plan = LessonPlan { sections: vec![section("a", "Isi satu."), LessonPlanSection { title: String::new(), ..section("b", "") }], ..Default::default() };
        assert_eq!(to_alm(&plan), "## Judul\n\nIsi satu.\n\n## Bagian 2");
    }

    #[test]
    fn sanitize_keeps_valid_content_verbatim() {
        let src = "Paragraf **tebal**.\n\n:::callout\nvariant: tip\ntext: Ingat ini.\n:::";
        assert_eq!(sanitize_generated(src), src);
    }

    #[test]
    fn sanitize_demotes_an_invalid_block_to_prose() {
        let src = "Awal.\n\n:::callout\nvariant: loud\ntext: Jangan hilang.\n:::\n\nAkhir.";
        let out = sanitize_generated(src);
        assert!(out.contains("Jangan hilang."), "{out}");
        assert!(!out.contains(":::"), "{out}");
        validate_content(&out).unwrap();
    }

    #[test]
    fn a_table_titled_instead_of_captioned_is_repaired_not_demoted() {
        // The schema has `caption`; models write `title`. Demoting cost
        // a whole table over one word — and put its JSON on the page.
        let src = ":::table\ntitle: Nilai Tempat\nheaders: [\"Nama\", \"Posisi\"]\nrows: [[\"Ratusan\", \"Ke-3\"]]\n:::";
        let out = sanitize_generated(src);
        assert!(out.contains(":::table"), "{out}");
        assert!(out.contains("caption: Nilai Tempat"), "{out}");
        let blocks = alm_parser::parse(&out).unwrap();
        assert!(block_schema::validate(&blocks[0].r#type, &blocks[0].data).is_ok());
    }

    #[test]
    fn a_demoted_block_never_puts_json_on_the_page() {
        // `variant: kuning` is not repairable, so this one really is
        // demoted — but the rows must come out as readable lines.
        let src = ":::comparison\nleft_label: A\nright_label: B\nrows: [[\"satu\"]]\n:::";
        let out = sanitize_generated(src);
        assert!(!out.contains('['), "{out}");
        assert!(!out.contains('"'), "{out}");
        assert!(out.contains("- satu"), "{out}");
    }

    #[test]
    fn sanitize_rescues_the_words_of_a_ragged_table() {
        // Ragged rows can't be repaired into a table, so this demotes —
        // but it demotes to readable lines, not to JSON.
        let src = ":::table\nheaders: [\"A\", \"B\"]\nrows: [[\"1\"]]\n:::";
        let out = sanitize_generated(src);
        assert!(out.contains("- A"), "{out}");
        assert!(out.contains("- 1"), "{out}");
        assert!(!out.contains('['), "{out}");
        validate_content(&out).unwrap();
    }

    #[test]
    fn sanitize_strips_invented_embeds() {
        let out = sanitize_generated(":::question_embed\nquestion_id: 00000000-0000-0000-0000-000000000000\n:::");
        assert!(!out.contains("question_embed"), "{out}");
    }

    #[test]
    fn sanitize_recovers_an_unterminated_directive() {
        let out = sanitize_generated("Awal.\n\n:::callout\nvariant: tip\ntext: Terpotong");
        assert!(out.contains("Terpotong"), "{out}");
        validate_content(&out).unwrap();
    }

    #[test]
    fn source_hash_changes_with_content() {
        let a = LessonPlan { sections: vec![section("a", "Satu.")], ..Default::default() };
        let b = LessonPlan { sections: vec![section("a", "Dua.")], ..Default::default() };
        assert_ne!(source_hash(&a), source_hash(&b));
        assert_eq!(source_hash(&a), source_hash(&a.clone()));
    }
}
