// `bab_content` — filling a bab (already structured by `bab_plan`) with
// what a learner needs: a Modul Belajar, a checkpoint pool per section,
// and a 50-question bank the two shorter Latihan draw from. A Rust port
// of `agent/tools/content-gen/generate_bab_content.py`'s orchestration,
// calling the SAME generation services the Studio editor calls
// (`lesson_plan_ai::generate_plan`, `quiz_generation::generate_quiz_group`)
// directly instead of over HTTP.
//
// Token discipline carries over from the Python original: the bank's
// plan is computed once (`blueprint.rs`), every fill call is told
// exactly which slots it owns, and the database — not an in-memory
// count — is read before every chunk, so a resumed task never
// re-requests what already landed.

use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::ai::{GenerateLessonPlanRequest, QuestionSlotRequest};
use crate::services::ai_provider::AIProvider;
use crate::services::lesson_plan::{self, LessonPlan, LessonPlanSection};
use crate::services::module_item;
use crate::services::quiz_generation::{self, GenerationMode, QuizGenerationBlueprint};
use crate::services::storage::AssetStorage;
use crate::Config;

use super::blueprint;

/// What one bab needs told about itself before anything is generated —
/// the brief a `bab_plan` topic carries in `metadata.bab_plan`, or
/// (topics from before that pipeline existed) just its title and
/// position among siblings. `bab_plan.v1`'s "Nilai Tempat: Satuan
/// sampai Jutaan" — the one bab this platform has actually shipped —
/// was written from exactly the empty-`notes` case.
#[derive(Debug, Clone)]
pub struct BabBrief {
    pub bab_title: String,
    pub topic_title: String,
    /// e.g. "Tahap 1 — Matematika Dasar" or, with an explicit jenjang,
    /// "Tahap 2 — Mekanika (SMA) · jenjang SMA/MA kelas 10-11 (Fase E-F)".
    pub level: String,
    pub language: String,
    pub jenis_soal: String,
    /// Every bab title in the topic, in order.
    pub siblings: Vec<String>,
    /// This bab's 0-based index into `siblings`.
    pub position: usize,
    /// tujuan + cakupan, when a `bab_plan` brief exists. Empty is valid.
    pub notes: String,
}

/// Real IDs the generator writes into — either the bab's real items (a
/// production run) or a scratch section built for one benchmark
/// comparison and deleted after (see `benchmark.rs`).
#[derive(Debug, Clone)]
pub struct BabItems {
    pub section_id: Uuid,
    pub module_id: Uuid,
    pub article_id: Uuid,
    pub bank_id: Uuid,
}

pub struct GenerateOutcome {
    pub lesson_plan: LessonPlan,
    pub bank_filled: usize,
    pub bank_deficit: usize,
    pub tokens: i64,
}

fn boundary_note(brief: &BabBrief) -> String {
    let others: Vec<String> = brief.siblings.iter().enumerate().filter(|(i, _)| *i != brief.position).map(|(i, t)| format!("{}. {t}", i + 1)).collect();
    format!(
        "Bab ini adalah bab ke-{} dari {} dalam topik \"{}\".\nBab LAIN dalam topik ini (masing-masing ditulis terpisah — JANGAN mengajarkannya di sini):\n{}\n\n\
         Batas materi: tulis HANYA porsi bab ini. Bila konsep milik bab lain terpaksa disinggung sebagai prasyarat, sebut satu kalimat lalu lanjut — jangan membuat bagian tersendiri untuknya.",
        brief.position + 1,
        brief.siblings.len(),
        brief.topic_title,
        others.join("\n"),
    )
}

/// 1. The Modul Belajar — one call, told which bab its siblings cover
/// so it stays inside its own boundary (the pilot's bab 1 wrote
/// sections on three other bab because nothing told it where they
/// were).
pub async fn generate_materi(pool: &PgPool, ai: &dyn AIProvider, model: &str, ctx: &AuthContext, article_id: Uuid, brief: &BabBrief) -> Result<(LessonPlan, i64), AppError> {
    let notes = format!(
        "Tulis sebagai modul belajar mandiri yang utuh untuk bab ini: mulai dari konsep, contoh bertahap, kesalahan umum, lalu rangkuman.\n\n{}{}",
        if brief.notes.trim().is_empty() { String::new() } else { format!("{}\n\n", brief.notes) },
        boundary_note(brief),
    );
    let req = GenerateLessonPlanRequest {
        item_id: article_id,
        topic: format!("{} (bagian dari topik \"{}\")", brief.bab_title, brief.topic_title),
        duration_minutes: 45,
        level: Some(brief.level.clone()),
        section_count: None,
        language: brief.language.clone(),
        notes: Some(notes),
        model: None,
    };
    let resp = crate::services::lesson_plan_ai::generate_plan(pool, ai, model, ctx, req).await?;
    let tokens = sqlx::query_scalar!(r#"select tokens_used from ai_tasks where id = $1"#, resp.ai_task_id).fetch_optional(pool).await?.flatten().unwrap_or(0);
    let saved = module_item::update_lesson_plan(pool, ctx, article_id, serde_json::to_value(&resp.lesson_plan).map_err(|e| AppError::Internal(e.into()))?, true).await?;
    let plan = lesson_plan::parse(&saved.lesson_plan)?;
    Ok((plan, tokens as i64))
}

/// One generate call, split smaller when the model's reply is cut off —
/// the model didn't fail, the budget did, so the retry asks for less,
/// never the same request that just failed. Returns (made, dropped).
#[allow(clippy::too_many_arguments)]
async fn generate_slots(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    model: &str,
    ctx: &AuthContext,
    item_id: Uuid,
    group_id: &str,
    slots: &[blueprint::Slot],
    context_prompt: &str,
    reference_item_id: Uuid,
    reference_section_id: Option<&str>,
) -> Result<(usize, usize, i64), AppError> {
    let mut made = 0usize;
    let mut dropped = 0usize;
    let mut tokens = 0i64;
    let mut queue: Vec<Vec<blueprint::Slot>> = vec![slots.to_vec()];
    while let Some(batch) = queue.pop() {
        if batch.is_empty() {
            continue;
        }
        let bp = QuizGenerationBlueprint {
            item_id,
            group_id: group_id.to_string(),
            mode: GenerationMode::Append,
            question_number: None,
            count: batch.len() as i64,
            context_prompt: Some(context_prompt.to_string()),
            reference_module_item_ids: vec![reference_item_id],
            asset_id: None,
            raw_text: None,
            convert_to_subtype: None,
            mark_draft: true,
            slots: batch.iter().map(|s| QuestionSlotRequest { bloom: s.bloom.clone(), difficulty: s.difficulty.clone(), section_id: s.section_id.clone() }).collect(),
            reference_section_id: reference_section_id.map(str::to_string),
        };
        match quiz_generation::generate_quiz_group(pool, config, ai, storage, ctx, model, bp).await {
            Ok(resp) => {
                made += batch.len().saturating_sub(resp.dropped_duplicates);
                dropped += resp.dropped_duplicates;
                let call_tokens = sqlx::query_scalar!(r#"select tokens_used from ai_tasks where id = $1"#, resp.ai_task_id).fetch_optional(pool).await?.flatten().unwrap_or(0);
                tokens += call_tokens as i64;
            }
            Err(e) => {
                let message = format!("{e:?}");
                if message.contains("MAX_TOKENS") && batch.len() > 1 {
                    let half = (batch.len() / 2).max(1);
                    let (a, b) = batch.split_at(half);
                    queue.push(a.to_vec());
                    queue.push(b.to_vec());
                    continue;
                }
                return Err(e);
            }
        }
    }
    Ok((made, dropped, tokens))
}

/// 2. Checkpoint pools. The generator only writes into quiz items, and a
/// checkpoint lives inside the plan — so it's generated into a scratch
/// quiz item (one group per section still under `CHECKPOINT_POOL`),
/// moved into the plan, and the scratch item deleted.
#[allow(clippy::too_many_arguments)]
pub async fn generate_checkpoints(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    model: &str,
    ctx: &AuthContext,
    module_id: Uuid,
    parent_id: Uuid,
    article_id: Uuid,
    bab_title: &str,
    mut plan: LessonPlan,
) -> Result<(LessonPlan, i64), AppError> {
    fn pool_size(section: &LessonPlanSection) -> usize {
        section
            .checkpoint
            .as_ref()
            .and_then(|v| v.get("question_groups"))
            .and_then(Value::as_array)
            .and_then(|groups| groups.first())
            .and_then(|g| g.get("questions"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0)
    }
    let needy: Vec<usize> = (0..plan.sections.len()).filter(|&i| pool_size(&plan.sections[i]) < blueprint::CHECKPOINT_POOL as usize).collect();
    if needy.is_empty() {
        return Ok((plan, 0));
    }

    let scratch_id = super::apply::create_item(pool, ctx, module_id, Some(parent_id), "item", &format!("(sementara) Checkpoint — {bab_title}"), Some("quiz"), None).await?;
    let groups: Vec<Value> = (0..needy.len()).map(|i| json!({"group_id": format!("cp{i}"), "type": blueprint::CHECKPOINT_SUBTYPE, "instruction": "Pilih jawaban yang tepat.", "questions": []})).collect();
    let mut tokens = 0i64;
    let mut generation_error = None;
    if let Err(e) = module_item::update_quiz_config(pool, ctx, scratch_id, json!({"question_groups": groups})).await {
        generation_error = Some(e);
    } else {
        for (gi, &si) in needy.iter().enumerate() {
            let section = &plan.sections[si];
            let wanted = blueprint::CHECKPOINT_POOL as usize - pool_size(section);
            let slots: Vec<_> = blueprint::checkpoint_blueprint(&section.id).into_iter().take(wanted).collect();
            let context = format!("Soal pengecekan pemahaman untuk bagian \"{}\" dari bab \"{bab_title}\". Uji apakah siswa memahami isi bagian ini, bukan bagian lain.", section.title);
            match generate_slots(pool, config, ai, storage, model, ctx, scratch_id, &format!("cp{gi}"), &slots, &context, article_id, Some(&section.id)).await {
                Ok((_, _, t)) => tokens += t,
                Err(e) => {
                    generation_error = Some(e);
                    break;
                }
            }
        }
    }

    // Merge whatever landed, even on a partial failure — a checkpoint
    // pool that's 3/4 filled is still worth keeping, and the next round
    // tops up the rest instead of losing what a failed call already paid
    // tokens for.
    let written: Option<Value> = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, scratch_id).fetch_optional(pool).await?.flatten();
    if let Some(written) = written {
        if let Some(groups) = written.get("question_groups").and_then(Value::as_array) {
            for (gi, &si) in needy.iter().enumerate() {
                let Some(group) = groups.iter().find(|g| g.get("group_id").and_then(Value::as_str) == Some(&format!("cp{gi}"))) else { continue };
                let Some(fresh) = group.get("questions").and_then(Value::as_array) else { continue };
                if fresh.is_empty() {
                    continue;
                }
                let section = &mut plan.sections[si];
                let mut existing: Vec<Value> = section
                    .checkpoint
                    .as_ref()
                    .and_then(|v| v.get("question_groups"))
                    .and_then(Value::as_array)
                    .and_then(|g| g.first())
                    .and_then(|g| g.get("questions"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                existing.extend(fresh.iter().cloned());
                for (n, q) in existing.iter_mut().enumerate() {
                    q["number"] = json!(n + 1);
                }
                let group_type = group.get("type").cloned().unwrap_or(json!(blueprint::CHECKPOINT_SUBTYPE));
                section.checkpoint = Some(json!({"question_groups": [{"group_id": "cp", "type": group_type, "questions": existing}]}));
            }
        }
    }
    module_item::delete(pool, ctx, scratch_id).await?;
    if let Some(e) = generation_error {
        return Err(e);
    }
    module_item::update_lesson_plan(pool, ctx, article_id, serde_json::to_value(&plan).map_err(|e| AppError::Internal(e.into()))?, true).await?;
    Ok((plan, tokens))
}

/// Ensures the bank's groups exist per the bab's `jenis_soal` mix
/// (idempotent — a group already there is left alone).
pub async fn ensure_bank_groups(pool: &PgPool, ctx: &AuthContext, bank_id: Uuid, level: &str, jenis_soal: &str) -> Result<(), AppError> {
    let raw: Option<Value> = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, bank_id).fetch_optional(pool).await?.flatten();
    let mut config = raw.unwrap_or_else(|| json!({}));
    let have: std::collections::HashSet<String> =
        config.get("question_groups").and_then(Value::as_array).map(|g| g.iter().filter_map(|x| x.get("group_id").and_then(Value::as_str)).map(str::to_string).collect()).unwrap_or_default();
    let mut groups = config.get("question_groups").and_then(Value::as_array).cloned().unwrap_or_default();
    const INSTRUCTIONS: [(&str, &str); 4] =
        [("multiple_choice", "Pilih jawaban yang paling tepat."), ("true_false", "Tentukan benar atau salah."), ("short_answer", "Jawab singkat — kerjakan sendiri."), ("multiple_choice_multiple", "Pilih SEMUA jawaban yang benar.")];
    for (subtype, _) in blueprint::bank_mixes(jenis_soal) {
        let gid = blueprint::subtype_group(subtype);
        if !have.contains(gid) {
            let instruction = INSTRUCTIONS.iter().find(|(s, _)| *s == subtype).map(|(_, i)| *i).unwrap_or("");
            groups.push(json!({"group_id": gid, "type": subtype, "instruction": instruction, "questions": []}));
        }
    }
    config["question_groups"] = json!(groups);
    config["level"] = json!(level);
    config["passing_score"] = json!(super::apply::PASSING_SCORE);
    config["shuffle_choices"] = json!(true);
    config["shuffle_question_order"] = json!(true);
    module_item::update_quiz_config(pool, ctx, bank_id, config).await?;
    Ok(())
}

/// What the bank actually holds right now, as blueprint keys.
async fn stored_slots(pool: &PgPool, bank_id: Uuid) -> Result<Vec<blueprint::Stored>, AppError> {
    let raw: Option<Value> = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, bank_id).fetch_optional(pool).await?.flatten();
    let Some(config) = raw else { return Ok(vec![]) };
    let mut out = Vec::new();
    for group in config.get("question_groups").and_then(Value::as_array).into_iter().flatten() {
        let group_id = group.get("group_id").and_then(Value::as_str).unwrap_or("").to_string();
        for q in group.get("questions").and_then(Value::as_array).into_iter().flatten() {
            let tax = q.get("taxonomy");
            let bloom = tax.and_then(|t| t.get("bloom")).and_then(Value::as_str).map(str::to_string);
            let difficulty = tax.and_then(|t| t.get("difficulty")).and_then(Value::as_str).map(str::to_string);
            let section_id = q.get("source_section_id").and_then(Value::as_str).map(str::to_string);
            out.push((group_id.clone(), bloom, difficulty, section_id));
        }
    }
    Ok(out)
}

/// 3. The bank — filled in chunks against the deterministic blueprint.
/// Later rounds ask for fewer questions at a time and (after two
/// rounds) widen the reference to the whole bab: a single ~270-word
/// section can't carry a fifth distinct C4 question about the same
/// narrow idea, and asking the same impossible thing again just lets
/// the duplicate filter throw away every retry.
#[allow(clippy::too_many_arguments)]
pub async fn fill_bank(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    model: &str,
    ctx: &AuthContext,
    bank_id: Uuid,
    article_id: Uuid,
    bab_title: &str,
    topic_title: &str,
    section_titles: &std::collections::HashMap<String, String>,
    plan_slots: &[blueprint::Slot],
    brief_notes: &str,
) -> Result<(usize, usize, i64, usize), AppError> {
    let mut dropped_total = 0usize;
    let mut tokens_total = 0i64;
    for attempt in 0..5usize {
        let existing = stored_slots(pool, bank_id).await?;
        let missing = blueprint::deficit(plan_slots, &existing);
        if missing.is_empty() {
            break;
        }
        let widened = attempt >= 2;
        let widened_slots: Vec<blueprint::Slot> = if widened { missing.into_iter().map(|s| blueprint::Slot { section_id: None, ..s }).collect() } else { missing };
        let chunk_size = if attempt == 0 { 5 } else { 2 };
        for chunk in blueprint::chunks(widened_slots, chunk_size) {
            let title = chunk.section_id.as_ref().and_then(|id| section_titles.get(id)).cloned().unwrap_or_else(|| "seluruh bab".to_string());
            let context = format!(
                "Soal latihan untuk bab \"{bab_title}\" (topik \"{topic_title}\"), bagian \"{title}\". Uji isi bagian itu, bukan materi bab lain.{}",
                if brief_notes.trim().is_empty() { String::new() } else { format!("\n\n{brief_notes}") }
            );
            match generate_slots(pool, config, ai, storage, model, ctx, bank_id, &chunk.group_id, &chunk.slots, &context, article_id, chunk.section_id.as_deref()).await {
                Ok((_, dropped, t)) => {
                    dropped_total += dropped;
                    tokens_total += t;
                }
                Err(_) => continue, // this chunk's slots stay in the deficit; the next round retries them
            }
        }
    }
    let stored = stored_slots(pool, bank_id).await?;
    let deficit_left = blueprint::deficit(plan_slots, &stored).len();
    Ok((stored.len(), dropped_total, tokens_total, deficit_left))
}

/// A small, representative slice of a bank for the judge to read — not
/// the whole thing, which would cost more to judge than to generate.
/// Spreads across groups rather than taking the first N of one subtype.
pub async fn sample_questions(pool: &PgPool, bank_id: Uuid, n: usize) -> Result<Vec<Value>, AppError> {
    let config: Option<Value> = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, bank_id).fetch_optional(pool).await?.flatten();
    let Some(config) = config else { return Ok(vec![]) };
    let mut out = Vec::new();
    for group in config.get("question_groups").and_then(Value::as_array).into_iter().flatten() {
        for q in group.get("questions").and_then(Value::as_array).into_iter().flatten().take(n / 3 + 1) {
            out.push(q.clone());
            if out.len() >= n {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// Creates the two pooled Latihan (draw 10 / draw 25 from the bank)
/// when they don't already exist — recognised by what they draw from
/// (`question_pool.source_item_id == bank_id`), not by title, so a
/// rename never creates a duplicate. A bab from before Latihan 1/2/3
/// pooling existed (one plain "Latihan" item, now repurposed as the
/// bank) gets brought up to the current shape the first time it's
/// generated through this pipeline; titles are also normalized to the
/// numbered form for the same reason.
pub async fn ensure_pooled_latihan(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, parent_id: Uuid, bab_title: &str, bank_id: Uuid, level: &str) -> Result<(), AppError> {
    let existing = sqlx::query!(r#"select id, quiz_config from module_items where parent_id = $1 and content_type = 'quiz' and id != $2"#, parent_id, bank_id).fetch_all(pool).await?;
    let has_draw = |count: i64| existing.iter().any(|e| e.quiz_config.as_ref().and_then(|q| q.get("question_pool")).and_then(|p| p.get("draw_count")).and_then(Value::as_i64) == Some(count));
    for &count in &super::apply::DRAW_COUNTS {
        if has_draw(count) {
            continue;
        }
        let ordinal = super::apply::DRAW_COUNTS.iter().position(|c| *c == count).expect("count is one of DRAW_COUNTS") + 1;
        let config = json!({
            "level": level, "question_groups": [], "passing_score": super::apply::PASSING_SCORE,
            "shuffle_choices": true, "shuffle_question_order": true,
            "question_pool": {"source_item_id": bank_id, "draw_count": count},
        });
        super::apply::create_item(pool, ctx, module_id, Some(parent_id), "item", &super::apply::latihan_title(ordinal, bab_title), Some("quiz"), Some(config)).await?;
    }
    // Titles normalized last, after the bank is guaranteed to have
    // something at the "Latihan {N}" slot to be numbered against.
    let want_bank_title = super::apply::latihan_title(super::apply::DRAW_COUNTS.len() + 1, bab_title);
    sqlx::query!(r#"update module_items set title = $2, updated_at = now() where id = $1 and title != $2"#, bank_id, want_bank_title).execute(pool).await?;
    Ok(())
}

/// The whole pipeline for one bab, in order: materi, then checkpoints
/// (which need the materi's section ids), then the bank (which
/// references the materi for context and the checkpoint-adjusted
/// section titles for the "which part is this testing" line).
#[allow(clippy::too_many_arguments)]
pub async fn generate_bab(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    lesson_model: &str,
    quiz_model: &str,
    ctx: &AuthContext,
    items: &BabItems,
    brief: &BabBrief,
) -> Result<GenerateOutcome, AppError> {
    let want_article_title = super::apply::pembahasan_title(&brief.bab_title);
    sqlx::query!(r#"update module_items set title = $2, updated_at = now() where id = $1 and title != $2"#, items.article_id, want_article_title).execute(pool).await?;
    let (plan, materi_tokens) = generate_materi(pool, ai, lesson_model, ctx, items.article_id, brief).await?;
    let (plan, checkpoint_tokens) = generate_checkpoints(pool, config, ai, storage, quiz_model, ctx, items.module_id, items.section_id, items.article_id, &brief.bab_title, plan).await?;

    // An article with checkpoints must be finished before its Latihan open.
    sqlx::query!(r#"update module_items set guard_config = $2, updated_at = now() where id = $1"#, items.article_id, json!({"completion_rule": "required"})).execute(pool).await?;

    ensure_bank_groups(pool, ctx, items.bank_id, &brief.level, &brief.jenis_soal).await?;
    ensure_pooled_latihan(pool, ctx, items.module_id, items.section_id, &brief.bab_title, items.bank_id, &brief.level).await?;
    let section_ids: Vec<String> = plan.sections.iter().map(|s| s.id.clone()).collect();
    let section_titles: std::collections::HashMap<String, String> = plan.sections.iter().map(|s| (s.id.clone(), s.title.clone())).collect();
    let plan_slots = blueprint::bank_blueprint(&section_ids, &brief.level, &brief.jenis_soal);
    let (bank_filled, _dropped, bank_tokens, bank_deficit) =
        fill_bank(pool, config, ai, storage, quiz_model, ctx, items.bank_id, items.article_id, &brief.bab_title, &brief.topic_title, &section_titles, &plan_slots, &brief.notes).await?;

    // Article, then the Latihan shortest first, bank last — same fixed
    // order `apply::build_bab` gives a freshly planned bab; here it
    // matters more, since `ensure_pooled_latihan` may just have created
    // the pooled items after this bab already existed in some other order.
    let siblings = sqlx::query!(r#"select id from module_items where parent_id = $1 order by order_index"#, items.section_id).fetch_all(pool).await?;
    let pool_order: Vec<Uuid> = {
        let rows = sqlx::query!(r#"select id, quiz_config from module_items where parent_id = $1 and content_type = 'quiz' and id != $2"#, items.section_id, items.bank_id).fetch_all(pool).await?;
        let mut rows: Vec<_> = rows.into_iter().filter_map(|r| Some((r.id, r.quiz_config?.get("question_pool")?.get("draw_count")?.as_i64()?))).collect();
        rows.sort_by_key(|(_, draw)| *draw);
        rows.into_iter().map(|(id, _)| id).collect()
    };
    let mut ordered = vec![items.article_id];
    ordered.extend(pool_order);
    ordered.push(items.bank_id);
    ordered.retain(|id| siblings.iter().any(|s| s.id == *id));
    let leftover: Vec<Uuid> = siblings.iter().map(|s| s.id).filter(|id| !ordered.contains(id)).collect();
    ordered.extend(leftover);
    module_item::reorder(pool, ctx, crate::models::requests::module::ReorderModuleItemsRequest { module_id: items.module_id, parent_id: Some(items.section_id), ordered_ids: ordered }).await?;

    Ok(GenerateOutcome { lesson_plan: plan, bank_filled, bank_deficit, tokens: materi_tokens + checkpoint_tokens + bank_tokens })
}
