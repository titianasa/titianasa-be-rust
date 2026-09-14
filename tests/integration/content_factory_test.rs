// Pabrik Konten (ADR-0015) `bab_plan` — the run → task → generate →
// validate → QA → apply lifecycle, driven through the same service
// functions the dashboard and the worker call, with a ScriptedProvider
// standing in for Vertex so the test spends no real tokens.
//
// DoD covered here: a clean plan that passes validation and clears the
// QA threshold is applied automatically (section + Pembahasan + Latihan
// 1/2/3 + `metadata.bab_plan`); a run with no curriculum standard is
// refused before any AI call; a learner-role context cannot reach any
// Admin endpoint.

use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use titian_backend_rust::models::auth::AuthContext;
use titian_backend_rust::services::ai_provider::ScriptedProvider;
use titian_backend_rust::services::content_factory::{admin, runner};
use titian_backend_rust::services::job_queue::JobContext;
use titian_backend_rust::Config;

const JWT_SECRET: &str = "test-jwt-secret";

fn test_config() -> Config {
    Config {
        bind_addr: "0.0.0.0:0".into(),
        database_url: String::new(),
        jwt_access_secret: JWT_SECRET.into(),
        access_token_ttl_minutes: 15,
        refresh_token_ttl_days: 30,
        frontend_origin: "http://localhost:3001".into(),
        google_client_id: "test-client-id".into(),
        mastery_confidence_threshold: 0.6,
        weakness_score_threshold: 60.0,
        rescue_mode_consecutive_failures: 3,
        review_queue_default_limit: 10,
        review_queue_min_gap_hours: 4,
        attendance_min_duration_ratio: 0.75,
        attendance_late_join_minutes: 10,
        ai_stt_model: "openai/whisper-1".into(),
        ai_tts_default_voice: "af_bella".into(),
        ai_speaking_room_text_model: "test-model".into(),
        ai_speaking_room_tts_model: "test-tts-model".into(),
        asset_max_bytes: 25 * 1024 * 1024,
        asset_signed_url_ttl_seconds: 3600,
        asset_presigned_put_ttl_seconds: 900,
        asset_public_signed_url_ttl_seconds: 604_800,
        mastery_lambda: 0.05,
        mastery_n_min: 5.0,
        frss_recalled_threshold: 0.8,
        frss_partial_threshold: 0.4,
        module_completion_min_accuracy: 80.0,
        module_completion_skip_credit_cost: 15,
        ai_writing_evaluation_model: "test-model".into(),
        ai_speaking_evaluation_model: "test-model".into(),
        ai_grammar_evaluation_credit_cost: 1,
        ai_grammar_evaluation_model: "test-model".into(),
        ai_lesson_generation_model: "test-model".into(),
        ai_question_generation_model: "test-model".into(),
        ai_ocr_model: "test-model".into(),
        ai_tts_model: "test-model".into(),
        redis_url: "redis://127.0.0.1:6379".into(),
        collab_checkpoint_interval_seconds: 15,
        ai_live_chat_model: "test-model".into(),
        gcp_project_id: "test-gcp-project".into(),
        gcp_region: "us-central1".into(),
        consent_guardian_confirmation_required: false,
    }
}

/// The real "Semua Mata Pelajaran" root — migration 0032 seeds it with
/// this fixed id in every database, including a fresh `#[sqlx::test]` one.
const LIBRARY_ROOT: Uuid = uuid::uuid!("5032dc43-03ff-4bb4-806d-a31148400509");

struct Fixture {
    admin_user_id: Uuid,
    domain_id: Uuid,
    topic_a: Uuid,
    topic_b: Uuid,
}

async fn seed_fixture(pool: &PgPool) -> Fixture {
    let admin_user_id: Uuid = sqlx::query_scalar!(r#"insert into users (email, name) values ('cf-admin@example.com', 'Admin Uji') returning id"#).fetch_one(pool).await.unwrap();

    let subject_id: Uuid = sqlx::query_scalar!(r#"insert into subjects (code, name) values ('CFTEST', 'Fisika Uji') returning id"#).fetch_one(pool).await.unwrap();
    let subject_folder_id: Uuid =
        sqlx::query_scalar!(r#"insert into modules (is_folder, parent_id, title) values (true, $1, 'Fisika Uji') returning id"#, LIBRARY_ROOT).fetch_one(pool).await.unwrap();
    let tahap_id: Uuid =
        sqlx::query_scalar!(r#"insert into modules (is_folder, parent_id, title) values (true, $1, 'Tahap Uji — SMA') returning id"#, subject_folder_id).fetch_one(pool).await.unwrap();
    let domain_id: Uuid = sqlx::query_scalar!(r#"insert into modules (is_folder, parent_id, title) values (true, $1, 'Dinamika Uji') returning id"#, tahap_id).fetch_one(pool).await.unwrap();
    let topic_a: Uuid = sqlx::query_scalar!(
        r#"insert into modules (is_folder, parent_id, subject_id, title, order_index) values (false, $1, $2, 'Hukum I Newton', 1) returning id"#,
        domain_id,
        subject_id,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let topic_b: Uuid = sqlx::query_scalar!(
        r#"insert into modules (is_folder, parent_id, subject_id, title, order_index) values (false, $1, $2, 'Hukum II Newton', 2) returning id"#,
        domain_id,
        subject_id,
    )
    .fetch_one(pool)
    .await
    .unwrap();

    Fixture { admin_user_id, domain_id, topic_a, topic_b }
}

async fn set_standard(pool: &PgPool, tahap_folder_id: Uuid) {
    sqlx::query!(
        r#"insert into curriculum_standards (tahap_folder_id, jenjang, standar, bahasa, jenis_soal) values ($1, 'SMA/MA kelas 10 (Fase E)', $2, 'id', 'hitungan')"#,
        tahap_folder_id,
        &["Kurikulum Merdeka Fase E — Fisika".to_string()],
    )
    .execute(pool)
    .await
    .unwrap();
}

fn admin_ctx(user_id: Uuid) -> AuthContext {
    AuthContext { user_id, organization_id: None, role: Some("platform_admin".to_string()) }
}

fn learner_ctx() -> AuthContext {
    AuthContext { user_id: Uuid::new_v4(), organization_id: None, role: Some("siswa".to_string()) }
}

fn job_context(pool: PgPool, provider: ScriptedProvider) -> JobContext {
    let ai: Arc<dyn titian_backend_rust::services::ai_provider::AIProvider> = Arc::new(provider);
    JobContext { pool, config: Arc::new(test_config()), text_ai: ai.clone(), ai }
}

fn clean_generate_reply() -> String {
    // Two topics, four non-overlapping babs each, every rule in
    // validate.rs satisfied (see its `a_clean_plan_has_no_issues` case).
    serde_json::json!({
        "topik": [
            {
                "no": 1, "t": "Hukum I Newton", "jenis_soal": null,
                "bab": [
                    {"judul": "Konsep Kelembaman dan Gaya Seimbang", "tujuan": "Siswa dapat menjelaskan konsep kelembaman pada benda diam dan bergerak.", "cakupan": ["kelembaman", "gaya seimbang", "syarat diam"]},
                    {"judul": "Penerapan Hukum I pada Benda Diam", "tujuan": "Siswa dapat menganalisis gaya-gaya pada benda dalam keadaan setimbang.", "cakupan": ["diagram gaya", "benda diam", "resultan nol"]},
                    {"judul": "Kerangka Acuan Inersial", "tujuan": "Siswa dapat membedakan kerangka acuan inersial dan non-inersial.", "cakupan": ["kerangka inersial", "kerangka non-inersial", "contoh kasus"]},
                    {"judul": "Diagram Benda Bebas pada Bidang Datar", "tujuan": "Siswa dapat menggambar diagram benda bebas untuk benda pada bidang datar.", "cakupan": ["diagram benda bebas", "bidang datar", "gaya normal"]},
                ],
            },
            {
                "no": 2, "t": "Hukum II Newton", "jenis_soal": null,
                "bab": [
                    {"judul": "Hubungan Gaya Massa dan Percepatan", "tujuan": "Siswa dapat menghitung percepatan benda dari resultan gaya dan massa.", "cakupan": ["F sama dengan ma", "resultan gaya", "satuan SI"]},
                    {"judul": "Penerapan Hukum II pada Sistem Katrol", "tujuan": "Siswa dapat menghitung percepatan dan tegangan tali pada sistem katrol.", "cakupan": ["sistem katrol", "tegangan tali", "dua benda terhubung"]},
                    {"judul": "Gaya Gesek dan Resultan Gaya", "tujuan": "Siswa dapat menghitung percepatan benda dengan memperhitungkan gaya gesek.", "cakupan": ["gaya gesek statis", "gaya gesek kinetis", "koefisien gesek"]},
                    {"judul": "Berat Semu di Dalam Lift", "tujuan": "Siswa dapat menghitung berat semu benda dalam lift yang dipercepat.", "cakupan": ["berat semu", "lift dipercepat", "gaya normal lift"]},
                ],
            },
        ]
    })
    .to_string()
}

fn passing_qa_reply() -> String {
    let dims = serde_json::json!({
        "kesesuaian_standar": 4.6, "urutan": 4.5, "granularitas": 4.5, "tanpa_tumpang_tindih": 4.7,
        "kelengkapan": 4.5, "kegunaan_jalur": 4.4, "kejelasan_tujuan": 4.6,
    });
    serde_json::json!({
        "topik": [
            {"no": 1, "skor": dims, "isu": []},
            {"no": 2, "skor": dims, "isu": []},
        ]
    })
    .to_string()
}

#[sqlx::test]
async fn a_clean_plan_that_passes_qa_is_applied_with_pembahasan_and_three_latihan(pool: PgPool) {
    let fx = seed_fixture(&pool).await;
    set_standard(&pool, sqlx::query_scalar!(r#"select parent_id from modules where id = $1"#, fx.domain_id).fetch_one(&pool).await.unwrap().unwrap()).await;
    let ctx = admin_ctx(fx.admin_user_id);

    let created = admin::create_run(
        &pool,
        &ctx,
        admin::CreateRunRequest { kind: "bab_plan".into(), title: None, domain_ids: vec![fx.domain_id], topic_ids: None, options: Default::default(), token_limit: None, benchmark: false },
    )
    .await
    .unwrap();
    assert!(created.skipped_domains.is_empty(), "{:?}", created.skipped_domains);
    assert_eq!(created.run.total_tasks, 1);

    let detail = admin::get_run(&pool, &ctx, created.run.id).await.unwrap();
    assert_eq!(detail.tasks.len(), 1);
    let task_id = detail.tasks[0].id;

    let jc = job_context(pool.clone(), ScriptedProvider::new(vec![clean_generate_reply(), passing_qa_reply()]));
    let outcome = runner::run_bab_plan(&jc, task_id).await.unwrap();
    assert!(matches!(outcome, runner::Outcome::Done));

    let task = admin::get_task(&pool, &ctx, task_id).await.unwrap();
    assert_eq!(task.status, "applied", "draft: {:?} validation: {:?} qa: {:?} error: {:?}", task.draft, task.validation, task.qa, task.error);

    for topic_id in [fx.topic_a, fx.topic_b] {
        let sections: Vec<(Uuid, String)> = sqlx::query!(r#"select id, title from module_items where module_id = $1 and parent_id is null and node_type = 'section' order by order_index"#, topic_id)
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.title))
            .collect();
        assert_eq!(sections.len(), 4, "topic {topic_id} should have 4 babs: {sections:?}");

        let (first_bab_id, first_bab_title) = &sections[0];
        let children: Vec<(String, String)> = sqlx::query!(r#"select node_type, title from module_items where parent_id = $1 order by order_index"#, first_bab_id)
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| (r.node_type, r.title))
            .collect();
        assert!(children.iter().any(|(t, title)| t == "item" && title == &format!("Pembahasan — {first_bab_title}")), "{children:?}");
        assert!(children.iter().any(|(_, title)| title == &format!("Latihan 1 — {first_bab_title}")), "{children:?}");
        assert!(children.iter().any(|(_, title)| title == &format!("Latihan 2 — {first_bab_title}")), "{children:?}");
        assert!(children.iter().any(|(_, title)| title == &format!("Latihan 3 — {first_bab_title}")), "{children:?}");

        let metadata: serde_json::Value = sqlx::query_scalar!(r#"select metadata as "metadata!" from modules where id = $1"#, topic_id).fetch_one(&pool).await.unwrap();
        let bab_plan = &metadata["bab_plan"];
        assert_eq!(bab_plan["author"], "gemini");
        assert_eq!(bab_plan["jenjang"], "SMA/MA kelas 10 (Fase E)");
        assert!(bab_plan["qa_average"].as_f64().unwrap() >= 4.2);
    }
}

#[sqlx::test]
async fn a_domain_with_no_curriculum_standard_is_skipped_before_any_ai_call(pool: PgPool) {
    let fx = seed_fixture(&pool).await;
    let ctx = admin_ctx(fx.admin_user_id);
    // No set_standard() call: the tahap has no curriculum_standards row.

    let err = admin::create_run(
        &pool,
        &ctx,
        admin::CreateRunRequest { kind: "bab_plan".into(), title: None, domain_ids: vec![fx.domain_id], topic_ids: None, options: Default::default(), token_limit: None, benchmark: false },
    )
    .await
    .unwrap_err();
    match err {
        titian_backend_rust::errors::AppError::UnprocessableEntity(code, detail) => {
            assert_eq!(code, "nothing_to_generate");
            assert!(detail.contains("standar kurikulum"), "{detail}");
        }
        other => panic!("expected UnprocessableEntity(nothing_to_generate, _), got {other:?}"),
    }

    let runs = admin::list_runs(&pool, &ctx).await.unwrap();
    assert!(runs.is_empty(), "no run should have been created: {runs:?}");
}

#[sqlx::test]
async fn a_learner_cannot_reach_any_content_factory_admin_call(pool: PgPool) {
    let fx = seed_fixture(&pool).await;
    set_standard(&pool, sqlx::query_scalar!(r#"select parent_id from modules where id = $1"#, fx.domain_id).fetch_one(&pool).await.unwrap().unwrap()).await;
    let learner = learner_ctx();

    assert!(admin::coverage(&pool, &learner).await.is_err());
    assert!(admin::list_runs(&pool, &learner).await.is_err());
    assert!(admin::create_run(
        &pool,
        &learner,
        admin::CreateRunRequest { kind: "bab_plan".into(), title: None, domain_ids: vec![fx.domain_id], topic_ids: None, options: Default::default(), token_limit: None, benchmark: false }
    )
    .await
    .is_err());
    assert!(admin::list_standards(&pool, &learner).await.is_err());

    // Sanity: the same calls succeed for platform_admin, so the failures
    // above are the permission gate, not a fixture mistake.
    let admin_ctx = admin_ctx(fx.admin_user_id);
    assert!(admin::coverage(&pool, &admin_ctx).await.is_ok());
}

// ── bab_content: run creation only (no AI call — see
// bin/benchmark_bab_content.rs and project memory for the real,
// live-verified generation path; scripting `lesson_plan_ai::generate_plan`
// and `quiz_generation::generate_quiz_group`'s own internal contracts
// faithfully is out of proportion to what this admin-layer logic needs
// covered) ──

/// A bab as `apply::build_bab` would have already made it: section,
/// Pembahasan article (optionally with real lesson_plan content), and a
/// bank quiz item with no `question_pool` (distinguishing it from a
/// pooled Latihan for `content_runner::load_context`'s own lookup).
async fn seed_bab(pool: &PgPool, topic_id: Uuid, order_index: i32, title: &str, with_materi: bool) -> Uuid {
    let section_id: Uuid = sqlx::query_scalar!(
        r#"insert into module_items (module_id, parent_id, node_type, title, order_index) values ($1, null, 'section', $2, $3) returning id"#,
        topic_id,
        title,
        order_index,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let lesson_plan = if with_materi {
        serde_json::json!({"title": title, "sections": [{"id": "s1", "title": "Bagian 1", "content": "Isi bagian satu.", "goal": ""}]})
    } else {
        serde_json::Value::Null
    };
    sqlx::query!(
        r#"insert into module_items (module_id, parent_id, node_type, title, content_type, lesson_plan) values ($1, $2, 'item', $3, 'article', $4)"#,
        topic_id,
        section_id,
        format!("Pembahasan — {title}"),
        lesson_plan,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query!(
        r#"insert into module_items (module_id, parent_id, node_type, title, content_type, quiz_config) values ($1, $2, 'item', $3, 'quiz', $4)"#,
        topic_id,
        section_id,
        format!("Latihan 3 — {title}"),
        serde_json::json!({"question_groups": []}),
    )
    .execute(pool)
    .await
    .unwrap();
    section_id
}

#[sqlx::test]
async fn bab_content_run_targets_only_babs_still_missing_materi(pool: PgPool) {
    let fx = seed_fixture(&pool).await;
    let ctx = admin_ctx(fx.admin_user_id);
    seed_bab(&pool, fx.topic_a, 1, "Bab Sudah Ada Materi", true).await;
    let bab_empty = seed_bab(&pool, fx.topic_a, 2, "Bab Belum Ada Materi", false).await;

    let created = admin::create_run(
        &pool,
        &ctx,
        admin::CreateRunRequest { kind: "bab_content".into(), title: None, domain_ids: vec![fx.topic_a], topic_ids: None, options: Default::default(), token_limit: None, benchmark: false },
    )
    .await
    .unwrap();
    assert_eq!(created.run.total_tasks, 1, "only the bab without materi should become a task");

    let detail = admin::get_run(&pool, &ctx, created.run.id).await.unwrap();
    assert_eq!(detail.tasks.len(), 1);
    let task = admin::get_task(&pool, &ctx, detail.tasks[0].id).await.unwrap();
    assert_eq!(task.kind, "bab_content");
    let target_id: Uuid = sqlx::query_scalar!(r#"select target_id from generation_tasks where id = $1"#, task.id).fetch_one(&pool).await.unwrap();
    assert_eq!(target_id, bab_empty);
}

#[sqlx::test]
async fn bab_content_run_can_be_restricted_to_specific_bab_ids(pool: PgPool) {
    let fx = seed_fixture(&pool).await;
    let ctx = admin_ctx(fx.admin_user_id);
    let bab_1 = seed_bab(&pool, fx.topic_a, 1, "Bab Satu", false).await;
    let _bab_2 = seed_bab(&pool, fx.topic_a, 2, "Bab Dua", false).await;

    let created = admin::create_run(
        &pool,
        &ctx,
        admin::CreateRunRequest { kind: "bab_content".into(), title: None, domain_ids: vec![fx.topic_a], topic_ids: Some(vec![bab_1]), options: Default::default(), token_limit: None, benchmark: false },
    )
    .await
    .unwrap();
    assert_eq!(created.run.total_tasks, 1, "topic_ids restricts to just the one bab id given, not every bab still missing materi");
}

#[sqlx::test]
async fn bab_content_run_is_refused_when_every_bab_already_has_materi(pool: PgPool) {
    let fx = seed_fixture(&pool).await;
    let ctx = admin_ctx(fx.admin_user_id);
    seed_bab(&pool, fx.topic_a, 1, "Bab Sudah Ada Materi", true).await;

    let err = admin::create_run(
        &pool,
        &ctx,
        admin::CreateRunRequest { kind: "bab_content".into(), title: None, domain_ids: vec![fx.topic_a], topic_ids: None, options: Default::default(), token_limit: None, benchmark: false },
    )
    .await
    .unwrap_err();
    match err {
        titian_backend_rust::errors::AppError::UnprocessableEntity(code, _) => assert_eq!(code, "nothing_to_generate"),
        other => panic!("expected nothing_to_generate, got {other:?}"),
    }
}

#[sqlx::test]
async fn bab_content_benchmark_mode_is_refused_with_a_clear_message(pool: PgPool) {
    let fx = seed_fixture(&pool).await;
    let ctx = admin_ctx(fx.admin_user_id);
    seed_bab(&pool, fx.topic_a, 1, "Bab", false).await;

    let err = admin::create_run(
        &pool,
        &ctx,
        admin::CreateRunRequest { kind: "bab_content".into(), title: None, domain_ids: vec![fx.topic_a], topic_ids: None, options: Default::default(), token_limit: None, benchmark: true },
    )
    .await
    .unwrap_err();
    match err {
        titian_backend_rust::errors::AppError::UnprocessableEntity(code, _) => assert_eq!(code, "benchmark_not_supported"),
        other => panic!("expected benchmark_not_supported, got {other:?}"),
    }
}
