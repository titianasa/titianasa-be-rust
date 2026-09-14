// Turning an accepted bab plan into the library structure — the Rust
// home of what `agent/tools/content-gen/apply_bab_plan.py` did from a
// developer's machine. Per bab, one section holding exactly:
//
//   Pembahasan — <bab>   article, completion required before the Latihan
//   Latihan 1 — <bab>    draws 10 from the bank
//   Latihan 2 — <bab>    draws 25 from the bank
//   Latihan 3 — <bab>    the bank itself (empty groups, mix per subject)
//
// and the plan stored on the topic (`modules.metadata.bab_plan`), which
// is what the content generator later writes each bab from. Idempotent:
// a bab title that already exists under the topic is left alone.

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::plan::{DomainContext, TopicDraft};
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::module::{CreateModuleItemRequest, ReorderModuleItemsRequest};
use crate::services::module_item;

pub const DRAW_COUNTS: [i64; 2] = [10, 25];
pub const PASSING_SCORE: i64 = 70;
const PLACEHOLDERS: [&str; 4] = ["Artikel 1", "Artikel 2", "Kuis 1", "Kuis 2"];

/// Bank groups per `jenis_soal`: (group id, subtype, instruction).
pub fn bank_groups(jenis_soal: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    match jenis_soal {
        "konsep" => vec![
            ("mc", "multiple_choice", "Pilih jawaban yang paling tepat."),
            ("tf", "true_false", "Tentukan benar atau salah."),
            ("mm", "multiple_choice_multiple", "Pilih SEMUA jawaban yang benar."),
        ],
        _ => vec![
            ("mc", "multiple_choice", "Pilih jawaban yang paling tepat."),
            ("tf", "true_false", "Tentukan benar atau salah."),
            ("sa", "short_answer", "Jawab singkat — kerjakan sendiri."),
        ],
    }
}

pub fn latihan_title(ordinal: usize, bab: &str) -> String {
    format!("Latihan {ordinal} — {bab}")
}

pub fn pembahasan_title(bab: &str) -> String {
    format!("Pembahasan — {bab}")
}

pub(super) async fn create_item(pool: &PgPool, ctx: &AuthContext, topic_id: Uuid, parent_id: Option<Uuid>, node_type: &str, title: &str, content_type: Option<&str>, quiz_config: Option<Value>) -> Result<Uuid, AppError> {
    let req = CreateModuleItemRequest {
        parent_id,
        node_type: node_type.to_string(),
        title: title.to_string(),
        content_type: content_type.map(str::to_string),
        content: None,
        format: None,
        concept_ids: None,
        quiz_config,
        subject_id: None,
    };
    Ok(module_item::create_with_provenance(pool, ctx, topic_id, req, "ai").await?.id)
}

async fn build_bab(pool: &PgPool, ctx: &AuthContext, topic_id: Uuid, bab_title: &str, jenis_soal: &str, level: &str) -> Result<Uuid, AppError> {
    let section_id = create_item(pool, ctx, topic_id, None, "section", bab_title, None, None).await?;
    let article_id = create_item(pool, ctx, topic_id, Some(section_id), "item", &pembahasan_title(bab_title), Some("article"), None).await?;

    let base = json!({"level": level, "passing_score": PASSING_SCORE, "shuffle_choices": true, "shuffle_question_order": true});
    let groups: Vec<Value> = bank_groups(jenis_soal).into_iter().map(|(id, subtype, instruction)| json!({"group_id": id, "type": subtype, "instruction": instruction, "questions": []})).collect();
    let mut bank_config = base.clone();
    bank_config["question_groups"] = json!(groups);
    let bank_id = create_item(pool, ctx, topic_id, Some(section_id), "item", &latihan_title(DRAW_COUNTS.len() + 1, bab_title), Some("quiz"), Some(bank_config)).await?;

    let mut ordered = vec![article_id];
    for (i, count) in DRAW_COUNTS.iter().enumerate() {
        let mut config = base.clone();
        config["question_groups"] = json!([]);
        config["question_pool"] = json!({"source_item_id": bank_id, "draw_count": count});
        ordered.push(create_item(pool, ctx, topic_id, Some(section_id), "item", &latihan_title(i + 1, bab_title), Some("quiz"), Some(config)).await?);
    }
    ordered.push(bank_id);

    // An article with checkpoints must be finished before its Latihan open.
    sqlx::query!(r#"update module_items set guard_config = $2, updated_at = now() where id = $1"#, article_id, json!({"completion_rule": "required"})).execute(pool).await?;
    module_item::reorder(pool, ctx, ReorderModuleItemsRequest { module_id: topic_id, parent_id: Some(section_id), ordered_ids: ordered }).await?;
    Ok(section_id)
}

/// The generic placeholders every topic was created with, removed only
/// when they are truly empty — never anything someone has written in.
async fn remove_empty_placeholders(pool: &PgPool, ctx: &AuthContext, topic_id: Uuid) -> Result<(), AppError> {
    let rows = sqlx::query!(
        r#"select i.id from module_items i
           where i.module_id = $1 and i.parent_id is null and i.node_type = 'item' and i.title = any($2) and i.status = 'draft'
             and coalesce(jsonb_array_length(i.lesson_plan->'sections'), 0) = 0
             and coalesce(jsonb_array_length(i.quiz_config->'question_groups'), 0) = 0
             and not exists (select 1 from content_blocks b where b.item_id = i.id)
             and not exists (select 1 from attempts a where a.item_id = i.id and not a.is_preview)"#,
        topic_id,
        &PLACEHOLDERS.map(str::to_string),
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        module_item::delete(pool, ctx, row.id).await?;
    }
    Ok(())
}

pub struct Applied {
    pub created_babs: usize,
}

/// Applies one topic's accepted plan. `approved_by` is recorded in the
/// stored plan (the run's owner for an automatic apply, the reviewer for
/// an approval).
pub async fn apply_topic(pool: &PgPool, ctx: &AuthContext, domain: &DomainContext, draft: &TopicDraft, task_id: Uuid, qa_average: Option<f64>, source: &str) -> Result<Applied, AppError> {
    let topic_id = draft.topic_id;
    let jenis_soal = draft.jenis_soal.clone().filter(|j| super::plan::JENIS_SOAL.contains(&j.as_str())).unwrap_or_else(|| domain.standard.jenis_soal.clone());
    let level = format!("{} · jenjang {}", domain.tahap_title, domain.standard.jenjang);

    remove_empty_placeholders(pool, ctx, topic_id).await?;
    let existing: Vec<(Uuid, String)> = sqlx::query!(r#"select id, title from module_items where module_id = $1 and parent_id is null and node_type = 'section'"#, topic_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.id, r.title))
        .collect();

    let mut created = 0;
    for bab in &draft.bab {
        if !existing.iter().any(|(_, t)| t == &bab.judul) {
            build_bab(pool, ctx, topic_id, &bab.judul, &jenis_soal, &level).await?;
            created += 1;
        }
    }

    // Bab order = plan order; anything else under the topic keeps its place after.
    let top: Vec<(Uuid, String, String)> = sqlx::query!(r#"select id, title, node_type from module_items where module_id = $1 and parent_id is null order by order_index"#, topic_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.id, r.title, r.node_type))
        .collect();
    let mut ordered: Vec<Uuid> = draft.bab.iter().filter_map(|b| top.iter().find(|(_, t, n)| n == "section" && t == &b.judul).map(|(id, _, _)| *id)).collect();
    let rest: Vec<Uuid> = top.iter().map(|(id, _, _)| *id).filter(|id| !ordered.contains(id)).collect();
    ordered.extend(rest);
    module_item::reorder(pool, ctx, ReorderModuleItemsRequest { module_id: topic_id, parent_id: None, ordered_ids: ordered }).await?;

    let topic = domain.topics.iter().find(|t| t.topic_id == topic_id);
    let plan = json!({
        "version": 1,
        "author": source,
        "planned_at": chrono::Utc::now().format("%Y-%m-%d").to_string(),
        "jenjang": domain.standard.jenjang,
        "bahasa": domain.standard.bahasa,
        "jenis_soal": jenis_soal,
        "standar": domain.standard.standar,
        "jalur": topic.map(|t| t.paths.clone()).unwrap_or_default(),
        "generation_task_id": task_id,
        "qa_average": qa_average,
        "bab": draft.bab,
    });
    sqlx::query!(
        r#"update modules set metadata = coalesce(metadata, '{}'::jsonb) || jsonb_build_object('bab_plan', $2::jsonb), updated_at = now() where id = $1"#,
        topic_id,
        plan,
    )
    .execute(pool)
    .await?;
    Ok(Applied { created_babs: created })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_mix_follows_jenis_soal() {
        let hitungan: Vec<&str> = bank_groups("hitungan").into_iter().map(|(_, s, _)| s).collect();
        assert_eq!(hitungan, vec!["multiple_choice", "true_false", "short_answer"]);
        let konsep: Vec<&str> = bank_groups("konsep").into_iter().map(|(_, s, _)| s).collect();
        assert!(konsep.contains(&"multiple_choice_multiple") && !konsep.contains(&"short_answer"));
        assert_eq!(latihan_title(3, "Hukum Hooke"), "Latihan 3 — Hukum Hooke");
    }
}
