// Pabrik Konten (ADR-0015), Phase 4 groundwork — a focused benchmark for
// the not-yet-built-out `bab_content` kind: generate ONE bab fresh, into
// a scratch section under the SAME topic as the one real bab this
// platform has shipped end to end (Matematika Tahap 1 › "Bilangan Cacah
// & Nilai Tempat" › "Nilai Tempat: Satuan sampai Jutaan" — reviewed and
// published by a human), then have a stronger judge compare the two
// blind. The scratch section is deleted after; nothing real is touched.
//
// Deliberately NOT wired into generation_runs/generation_tasks yet — the
// user asked to benchmark first, before the dashboard-integrated version
// (mirroring bab_plan's own "harness, then decide whether to build the
// rest" order). If this passes, that's the natural next ticket.
//
// Usage: `cargo run --bin benchmark_bab_content` (reads DATABASE_URL and
// GCP_PROJECT_ID/GCP_REGION the same way the server does).

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use titian_backend_rust::models::auth::AuthContext;
use titian_backend_rust::models::requests::module::CreateModuleItemRequest;
use titian_backend_rust::services::content_factory::{blueprint, content, content_qa};
use titian_backend_rust::services::storage::InMemoryStorage;
use titian_backend_rust::services::{ai_provider, ai_settings, lesson_plan, module_item, vertex_ai_provider};
use titian_backend_rust::Config;

// The one bab this platform has actually shipped end to end.
const TOPIC_ID: &str = "13656868-d09e-4ecf-899b-7052d81a2e55"; // Bilangan Cacah & Nilai Tempat
const GOLD_SECTION_ID: &str = "b0000000-0000-0000-0000-000000000000"; // placeholder, resolved by title below
const GOLD_BAB_TITLE: &str = "Nilai Tempat: Satuan sampai Jutaan";
const GOLD_ARTICLE_ID: &str = "a149d0cf-a334-4e21-8563-971c9b32de73";
const GOLD_BANK_ID: &str = "bfa9213d-1ba9-4555-9cce-c71ff29a7096";
const SIBLINGS: [&str; 7] = [
    "Mengenal Bilangan Cacah",
    "Nilai Tempat: Satuan sampai Jutaan",
    "Membaca & Menulis Lambang Bilangan",
    "Bentuk Panjang & Bentuk Baku Bilangan",
    "Membandingkan & Mengurutkan Bilangan Cacah",
    "Pembulatan & Taksiran Sederhana",
    "Kesalahan Umum: Tertukar Nilai Angka dengan Nilai Tempat",
];
const LEVEL: &str = "Tahap 1 — Matematika Dasar";
const SAMPLE_SIZE: usize = 8;

fn admin_ctx(user_id: Uuid) -> AuthContext {
    AuthContext { user_id, organization_id: None, role: Some("platform_admin".to_string()) }
}

async fn sample_questions(pool: &sqlx::PgPool, bank_id: Uuid, n: usize) -> anyhow::Result<Vec<serde_json::Value>> {
    let config: serde_json::Value = sqlx::query_scalar!(r#"select quiz_config as "c!" from module_items where id = $1"#, bank_id).fetch_one(pool).await?;
    let mut out = Vec::new();
    for group in config.get("question_groups").and_then(serde_json::Value::as_array).into_iter().flatten() {
        for q in group.get("questions").and_then(serde_json::Value::as_array).into_iter().flatten().take(n / 3 + 1) {
            out.push(q.clone());
            if out.len() >= n {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = titian_backend_rust::db::connect(&config.database_url).await?;
    let ai: Arc<dyn ai_provider::AIProvider> = Arc::new(vertex_ai_provider::VertexGeminiProvider::new(config.gcp_project_id.clone(), config.gcp_region.clone()).await?);
    let storage = InMemoryStorage::new();
    let admin_user_id: Uuid = "e0b7f9ab-228d-449d-8fd5-9b7cd4d51a77".parse()?; // ndsanja@gmail.com, platform_admin
    let ctx = admin_ctx(admin_user_id);
    let topic_id: Uuid = TOPIC_ID.parse()?;
    let _ = GOLD_SECTION_ID; // resolved by title, kept as a named constant for readability

    println!("=== Benchmark bab_content: \"{GOLD_BAB_TITLE}\" ===");

    // 1. Scratch section under the SAME topic, with the SAME (empty)
    // brief the gold bab actually got — a fair, blind comparison.
    let scratch_title = format!("(benchmark) {GOLD_BAB_TITLE}");
    let section_id = module_item::create_with_provenance(
        &pool,
        &ctx,
        topic_id,
        CreateModuleItemRequest { parent_id: None, node_type: "section".into(), title: scratch_title.clone(), content_type: None, content: None, format: None, concept_ids: None, quiz_config: None, subject_id: None },
        "ai",
    )
    .await?
    .id;
    let article_id = module_item::create_with_provenance(
        &pool,
        &ctx,
        topic_id,
        CreateModuleItemRequest { parent_id: Some(section_id), node_type: "item".into(), title: format!("Pembahasan — {scratch_title}"), content_type: Some("article".into()), content: None, format: None, concept_ids: None, quiz_config: None, subject_id: None },
        "ai",
    )
    .await?
    .id;
    let bank_id = module_item::create_with_provenance(
        &pool,
        &ctx,
        topic_id,
        CreateModuleItemRequest { parent_id: Some(section_id), node_type: "item".into(), title: format!("Latihan 3 — {scratch_title}"), content_type: Some("quiz".into()), content: None, format: None, concept_ids: None, quiz_config: Some(json!({"question_groups": []})), subject_id: None },
        "ai",
    )
    .await?
    .id;
    println!("scratch section {section_id} created");

    let skip_cleanup = std::env::var("SKIP_CLEANUP").is_ok();
    let cleanup = async {
        if skip_cleanup {
            println!("SKIP_CLEANUP set — leaving scratch section {section_id} for manual inspection; delete it yourself after.");
            return;
        }
        let _ = module_item::delete(&pool, &ctx, section_id).await;
    };

    let run = async {
        let lesson_model = ai_settings::resolve(&pool, &config, "lesson_generation").await?.model_id;
        let quiz_model = ai_settings::resolve(&pool, &config, "quiz_generation").await?.model_id;
        println!("generating with lesson_model={lesson_model} quiz_model={quiz_model}");

        let brief = content::BabBrief {
            bab_title: GOLD_BAB_TITLE.to_string(),
            topic_title: "Bilangan Cacah & Nilai Tempat".to_string(),
            level: LEVEL.to_string(),
            language: "id".to_string(),
            jenis_soal: "hitungan".to_string(),
            siblings: SIBLINGS.iter().map(|s| s.to_string()).collect(),
            position: 1,
            notes: String::new(),
        };
        let items = content::BabItems { section_id, module_id: topic_id, article_id, bank_id };

        let t0 = std::time::Instant::now();
        let outcome = content::generate_bab(&pool, &config, ai.as_ref(), &storage, &lesson_model, &quiz_model, &ctx, &items, &brief).await?;
        println!(
            "generated: {} bagian, bank {}/{} terisi (defisit {}), {} token, {:.0}s",
            outcome.lesson_plan.sections.len(),
            outcome.bank_filled,
            blueprint::BANK_SIZE,
            outcome.bank_deficit,
            outcome.tokens,
            t0.elapsed().as_secs_f64()
        );

        // 2. Read both sides fresh from the database (the generated one,
        // just written; the gold one, exactly as a learner sees it).
        let generated_plan_raw: serde_json::Value = sqlx::query_scalar!(r#"select lesson_plan as "p!" from module_items where id = $1"#, article_id).fetch_one(&pool).await?;
        let generated_plan = lesson_plan::parse(&generated_plan_raw)?;
        let gold_article_id: Uuid = GOLD_ARTICLE_ID.parse()?;
        let gold_bank_id: Uuid = GOLD_BANK_ID.parse()?;
        let gold_plan_raw: serde_json::Value = sqlx::query_scalar!(r#"select lesson_plan as "p!" from module_items where id = $1"#, gold_article_id).fetch_one(&pool).await?;
        let gold_plan = lesson_plan::parse(&gold_plan_raw)?;

        let generated_questions = sample_questions(&pool, bank_id, SAMPLE_SIZE).await?;
        let gold_questions = sample_questions(&pool, gold_bank_id, SAMPLE_SIZE).await?;
        println!("sampled {} generated soal, {} gold soal for the judge", generated_questions.len(), gold_questions.len());

        if std::env::var("DUMP_SAMPLE").is_ok() {
            println!("\n=== SAMPLE: generated materi, first section ===");
            if let Some(s) = generated_plan.sections.first() {
                println!("judul: {}\n{}", s.title, s.content);
            }
            println!("\n=== SAMPLE: generated soal (3) ===");
            for q in generated_questions.iter().take(3) {
                println!("{}\n", serde_json::to_string_pretty(q).unwrap_or_default());
            }
        }

        // 3. Judge, blind, label shuffled by a byte of the topic id (same
        // convention as the bab_plan benchmark).
        let generated_is_a = topic_id.as_bytes()[0] % 2 == 0;
        let gen_sample = content_qa::ContentSample { bab_title: GOLD_BAB_TITLE, level: LEVEL, plan: &generated_plan, sample_questions: &generated_questions };
        let gold_sample = content_qa::ContentSample { bab_title: GOLD_BAB_TITLE, level: LEVEL, plan: &gold_plan, sample_questions: &gold_questions };
        let (a, b) = if generated_is_a { (&gen_sample, &gold_sample) } else { (&gold_sample, &gen_sample) };
        let prompt = content_qa::pairwise_prompt(a, b);

        let judge_model_resolved = ai_settings::resolve(&pool, &config, "agent_qa").await?;
        let max_tokens = ai_provider::resolve_max_tokens(&pool, &judge_model_resolved.model_id, 32_000).await;
        let request = ai_provider::GenerationRequest {
            model: judge_model_resolved.model_id.clone(),
            system_prompt: content_qa::system_prompt(),
            user_prompt: prompt,
            temperature: judge_model_resolved.temperature.unwrap_or(0.1),
            max_tokens,
            image_url: None,
            json_mode: true,
            thinking_budget: Some(2048),
            allow_partial: false,
        };
        let (result, judge_model) = ai_provider::generate_with_fallback(ai.as_ref(), &judge_model_resolved, request).await;
        let resp = result.map_err(|e| anyhow::anyhow!("penilai gagal: {e}"))?;
        let judge_tokens = resp.tokens_used.unwrap_or(0);
        println!("judged by {judge_model}, {judge_tokens} token");

        let result = content_qa::parse_pairwise(&resp.text, generated_is_a).map_err(|e| anyhow::anyhow!(e))?;

        println!("\n=== HASIL ===");
        println!("lebih disukai: {}", result.preferred);
        println!("alasan: {}", result.reason);
        println!("generated rata-rata: {:.2}", result.generated.average());
        println!("gold      rata-rata: {:.2}", result.gold.average());
        for (d, _) in content_qa::DIMENSIONS {
            println!("  {d:22} gen={:.1} gold={:.1}", result.generated.skor.get(d).copied().unwrap_or(0.0), result.gold.skor.get(d).copied().unwrap_or(0.0));
        }
        if !result.generated.isu.is_empty() {
            println!("isu pada versi generated:");
            for i in &result.generated.isu {
                println!("  [{:?}] {}", i.tingkat, i.masalah);
            }
        }
        if !result.gold.isu.is_empty() {
            println!("isu pada versi gold:");
            for i in &result.gold.isu {
                println!("  [{:?}] {}", i.tingkat, i.masalah);
            }
        }
        println!("\ntotal token (generate + judge): {}", outcome.tokens + judge_tokens as i64);

        Ok::<(), anyhow::Error>(())
    }
    .await;

    cleanup.await;
    if !skip_cleanup {
        println!("scratch section {section_id} deleted");
    }
    run
}
