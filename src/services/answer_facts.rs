// Writer for `question_answer_facts` (migrations/0058): one row per
// graded answer, placed in the curriculum once per submission via
// content_context. Called from the two places a learner's answer is
// graded for real — a quiz submit and a checkpoint submit — and never
// from a preview.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::content_context;
use crate::services::quiz_config::QuizQuestion;

pub struct FactInput {
    pub question_uid: Uuid,
    pub content_item_id: Uuid,
    pub source_section_id: Option<String>,
    pub subtype: String,
    pub difficulty: Option<String>,
    pub bloom: Option<String>,
    pub correct: Option<bool>,
    pub points_earned: f64,
    pub points_max: f64,
    pub content_version: Option<i32>,
}

/// Difficulty and Bloom as the lowercase labels stored everywhere else
/// ("mudah", "c3").
pub fn taxonomy_labels(question: &QuizQuestion) -> (Option<String>, Option<String>) {
    let label = |v: Option<serde_json::Value>| v.and_then(|v| v.as_str().map(str::to_string));
    let taxonomy = question.taxonomy.as_ref();
    (
        label(taxonomy.and_then(|t| t.difficulty).and_then(|d| serde_json::to_value(d).ok())),
        label(taxonomy.and_then(|t| t.bloom).and_then(|b| serde_json::to_value(b).ok())),
    )
}

pub async fn record(pool: &PgPool, user_id: Uuid, module_item_id: Uuid, source: &str, attempt_id: Option<Uuid>, facts: Vec<FactInput>) -> Result<(), AppError> {
    if facts.is_empty() {
        return Ok(());
    }
    let Some(place) = content_context::resolve(pool, module_item_id).await? else { return Ok(()) };
    let folder_path: Vec<Uuid> = place.path.iter().map(|n| n.id).collect();
    let bab_id = place.bab.as_ref().map(|b| b.id);

    let mut uids = Vec::with_capacity(facts.len());
    let mut content_items = Vec::with_capacity(facts.len());
    let mut sections: Vec<Option<String>> = Vec::with_capacity(facts.len());
    let mut subtypes = Vec::with_capacity(facts.len());
    let mut difficulties: Vec<Option<String>> = Vec::with_capacity(facts.len());
    let mut blooms: Vec<Option<String>> = Vec::with_capacity(facts.len());
    let mut corrects: Vec<Option<bool>> = Vec::with_capacity(facts.len());
    let mut earned = Vec::with_capacity(facts.len());
    let mut maxes = Vec::with_capacity(facts.len());
    let mut versions: Vec<Option<i32>> = Vec::with_capacity(facts.len());
    for f in facts {
        uids.push(f.question_uid);
        content_items.push(f.content_item_id);
        sections.push(f.source_section_id);
        subtypes.push(f.subtype);
        difficulties.push(f.difficulty);
        blooms.push(f.bloom);
        corrects.push(f.correct);
        earned.push(f.points_earned as f32);
        maxes.push(f.points_max as f32);
        versions.push(f.content_version);
    }

    sqlx::query!(
        r#"insert into question_answer_facts
             (user_id, source, attempt_id, module_item_id, subject_id, folder_path, module_id, bab_id,
              question_uid, content_item_id, source_section_id, subtype, difficulty, bloom, correct, points_earned, points_max, content_version)
           select $1, $2, $3, $4, $5, $6, $7, $8, u.*
           from unnest($9::uuid[], $10::uuid[], $11::text[], $12::text[], $13::text[], $14::text[], $15::bool[], $16::real[], $17::real[], $18::int[])
             as u(question_uid, content_item_id, source_section_id, subtype, difficulty, bloom, correct, points_earned, points_max, content_version)"#,
        user_id,
        source,
        attempt_id,
        module_item_id,
        place.subject_id,
        &folder_path,
        place.module_id,
        bab_id,
        &uids,
        &content_items,
        &sections as &[Option<String>],
        &subtypes,
        &difficulties as &[Option<String>],
        &blooms as &[Option<String>],
        &corrects as &[Option<bool>],
        &earned,
        &maxes,
        &versions as &[Option<i32>],
    )
    .execute(pool)
    .await?;
    Ok(())
}
