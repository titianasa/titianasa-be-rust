// P40-003 (ADR-0013 §3.5, ADR-0014 §2 "Pengaturan AI") — every call
// site that used to read `state.config.ai_*_model` directly now asks
// `resolve(pool, config, role)` instead. Resolution order, per role:
//
//   1. an ENABLED row in `ai_role_settings` for that role,
//   2. else the role's `env_fallback` — the exact `Config` field that
//      call site read before this ticket (itself already "env override,
//      else a hardcoded default" from `Config::from_env`),
//   3. there is no step 3: every role has an env_fallback, so this
//      never needs a bare hardcoded literal here.
//
// This is what makes the ticket's own DoD true by construction: "tanpa
// baris di ai_role_settings, semua fitur AI berjalan persis seperti
// sekarang" — with an empty `ai_role_settings` table, `resolve` always
// takes branch 2, i.e. returns exactly what the caller used to read.
//
// Cached in-process (`snapshot`), invalidated by `invalidate_cache()` —
// called at the end of every write in this file — so a model change in
// Admin Pusat is live on the NEXT call, never stale until a restart.

use std::collections::HashMap;
use std::sync::OnceLock;

use sqlx::PgPool;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};

#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogEntry {
    pub id: Uuid,
    pub provider: String,
    pub model_id: String,
    pub label: String,
    pub capabilities: Vec<String>,
    pub max_output_tokens: Option<i32>,
    pub price_input_per_mtok_idr: Option<i32>,
    pub price_output_per_mtok_idr: Option<i32>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RoleSettingRow {
    pub role: String,
    pub model_id: Option<String>,
    pub fallback_model_id: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i32>,
    pub daily_token_budget: Option<i32>,
    pub enabled: bool,
    pub updated_by: Option<Uuid>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// One entry per AI role this codebase can call. `env_fallback` is the
/// role's step-2 source (see module doc) — for `quiz_generation`, which
/// has never had its own `Config` field, that's deliberately the SAME
/// field `lesson_generation` reads (`ai_lesson_generation_model`),
/// because that is genuinely what every quiz-generation call site
/// already falls back to today (`handlers/ai.rs`'s
/// `resolve_ai_model(body.model, &config.ai_lesson_generation_model)`).
/// The four `agent_*` roles have no live caller yet (Fase 42) and no
/// legacy `Config` field to preserve, so their fallback is the
/// platform's own current default model literal.
pub struct AiRoleInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub required_capabilities: &'static [&'static str],
    pub env_fallback: fn(&Config) -> &str,
}

fn agent_default(_: &Config) -> &'static str {
    "gemini-3.8-flash"
}

pub const AI_ROLES: &[AiRoleInfo] = &[
    AiRoleInfo { id: "lesson_generation", label: "Membuat & Mengedit Modul Belajar", required_capabilities: &["text", "json"], env_fallback: |c| &c.ai_lesson_generation_model },
    AiRoleInfo { id: "question_generation", label: "Membuat Soal (Bank Soal)", required_capabilities: &["text"], env_fallback: |c| &c.ai_question_generation_model },
    // No dedicated Config field — shares lesson_generation's fallback
    // today (see doc comment above).
    AiRoleInfo { id: "quiz_generation", label: "Membuat & Menulis Ulang Kuis", required_capabilities: &["text", "json"], env_fallback: |c| &c.ai_lesson_generation_model },
    AiRoleInfo { id: "ocr", label: "OCR ke Soal", required_capabilities: &["text", "vision"], env_fallback: |c| &c.ai_ocr_model },
    AiRoleInfo { id: "live_chat", label: "Obrolan AI (Live Chat)", required_capabilities: &["text"], env_fallback: |c| &c.ai_live_chat_model },
    AiRoleInfo { id: "writing_evaluation", label: "Menilai Tulisan (Esai)", required_capabilities: &["text", "json"], env_fallback: |c| &c.ai_writing_evaluation_model },
    AiRoleInfo { id: "speaking_evaluation", label: "Menilai Berbicara (Speaking)", required_capabilities: &["text", "json"], env_fallback: |c| &c.ai_speaking_evaluation_model },
    AiRoleInfo { id: "grammar_evaluation", label: "Menilai Tata Bahasa", required_capabilities: &["text", "json"], env_fallback: |c| &c.ai_grammar_evaluation_model },
    AiRoleInfo { id: "speaking_room_text", label: "Ruang Bicara — Teks", required_capabilities: &["text", "json"], env_fallback: |c| &c.ai_speaking_room_text_model },
    AiRoleInfo { id: "speaking_room_tts", label: "Ruang Bicara — Suara", required_capabilities: &["tts"], env_fallback: |c| &c.ai_speaking_room_tts_model },
    AiRoleInfo { id: "stt", label: "Ubah Suara ke Teks (STT)", required_capabilities: &["stt"], env_fallback: |c| &c.ai_stt_model },
    AiRoleInfo { id: "tts", label: "Ubah Teks ke Suara (TTS)", required_capabilities: &["tts"], env_fallback: |c| &c.ai_tts_model },
    // ADR-0013 agent roles — registered now, first caller is Fase 42.
    AiRoleInfo { id: "agent_diagnosis", label: "Agen AI — Diagnosis Konten", required_capabilities: &["text"], env_fallback: agent_default },
    AiRoleInfo { id: "agent_editor", label: "Agen AI — Editor Konten", required_capabilities: &["text"], env_fallback: agent_default },
    AiRoleInfo { id: "agent_qa", label: "Agen AI — QA Konten", required_capabilities: &["text"], env_fallback: agent_default },
    AiRoleInfo { id: "agent_reporter", label: "Agen AI — Pelapor", required_capabilities: &["text"], env_fallback: agent_default },
];

pub fn find_role(id: &str) -> Option<&'static AiRoleInfo> {
    AI_ROLES.iter().find(|r| r.id == id)
}

#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub model_id: String,
    pub fallback_model_id: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i32>,
}

struct Snapshot {
    catalog: Vec<CatalogEntry>,
    roles: HashMap<String, RoleSettingRow>,
}

/// Keyed by the pool's OWN database name, not a single bare slot — in
/// production there is exactly one `PgPool` for the process's whole
/// lifetime, so this degenerates to one entry, same cost as a bare
/// static. In the integration test suite, though, `#[sqlx::test]` gives
/// every test its own freshly-named database, and many tests run
/// CONCURRENTLY in one process — a bare process-wide cache with no key
/// would let one test's catalog write bleed into a completely unrelated
/// test reading a different database. Keying by database name is what
/// actually isolates them, at zero cost to the real single-pool case.
static CACHE: OnceLock<RwLock<HashMap<String, Snapshot>>> = OnceLock::new();

fn cache_lock() -> &'static RwLock<HashMap<String, Snapshot>> {
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn cache_key(pool: &PgPool) -> String {
    pool.connect_options().get_database().unwrap_or("default").to_string()
}

/// Discards this pool's cached snapshot — called at the end of every
/// write in this file, so the very next `resolve`/`catalog_list`/
/// `role_list` call on the SAME pool re-reads the database instead of
/// serving stale data. This is the whole mechanism behind the DoD's
/// "langsung berlaku di panggilan berikutnya tanpa restart".
pub async fn invalidate_cache(pool: &PgPool) {
    cache_lock().write().await.remove(&cache_key(pool));
}

async fn fetch_catalog(pool: &PgPool) -> Result<Vec<CatalogEntry>, AppError> {
    let rows = sqlx::query_as!(
        CatalogEntry,
        r#"select id, provider, model_id, label, capabilities as "capabilities!", max_output_tokens,
                  price_input_per_mtok_idr, price_output_per_mtok_idr, enabled, created_at
           from ai_model_catalog order by provider, model_id"#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn fetch_roles(pool: &PgPool) -> Result<HashMap<String, RoleSettingRow>, AppError> {
    let rows = sqlx::query_as!(
        RoleSettingRow,
        r#"select role, model_id, fallback_model_id, temperature as "temperature: f64", max_tokens, daily_token_budget, enabled, updated_by, updated_at
           from ai_role_settings"#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.role.clone(), r)).collect())
}

async fn snapshot(pool: &PgPool) -> Result<(Vec<CatalogEntry>, HashMap<String, RoleSettingRow>), AppError> {
    let key = cache_key(pool);
    {
        let read = cache_lock().read().await;
        if let Some(s) = read.get(&key) {
            return Ok((s.catalog.clone(), s.roles.clone()));
        }
    }
    let catalog = fetch_catalog(pool).await?;
    let roles = fetch_roles(pool).await?;
    cache_lock().write().await.insert(key, Snapshot { catalog: catalog.clone(), roles: roles.clone() });
    Ok((catalog, roles))
}

pub async fn catalog_list(pool: &PgPool) -> Result<Vec<CatalogEntry>, AppError> {
    Ok(snapshot(pool).await?.0)
}

pub async fn role_list(pool: &PgPool) -> Result<Vec<RoleSettingRow>, AppError> {
    let (_, roles) = snapshot(pool).await?;
    Ok(AI_ROLES
        .iter()
        .map(|r| {
            roles.get(r.id).cloned().unwrap_or_else(|| RoleSettingRow {
                role: r.id.to_string(),
                model_id: None,
                fallback_model_id: None,
                temperature: None,
                max_tokens: None,
                daily_token_budget: None,
                enabled: true,
                updated_by: None,
                updated_at: chrono::DateTime::UNIX_EPOCH,
            })
        })
        .collect())
}

/// The resolver every AI-calling service/handler uses instead of
/// reading `config.ai_*_model` directly. Never fails on an unknown
/// role by falling back to a default — an unknown `role` is a
/// programming error (a typo'd string literal at a call site), so this
/// panics in that one case rather than silently picking some model;
/// every real call site passes a `role` straight from a `const` in
/// `AI_ROLES`; a panic here is caught immediately in dev/tests, unlike
/// a wrong model silently being used in production.
pub async fn resolve(pool: &PgPool, config: &Config, role: &str) -> Result<ResolvedModel, AppError> {
    let info = find_role(role).unwrap_or_else(|| panic!("ai_settings::resolve: unknown role \"{role}\" — this is a bug at the call site, not user input"));
    let (_, roles) = snapshot(pool).await?;

    if let Some(setting) = roles.get(role) {
        if setting.enabled {
            if let Some(model_id) = &setting.model_id {
                return Ok(ResolvedModel {
                    model_id: model_id.clone(),
                    fallback_model_id: setting.fallback_model_id.clone(),
                    temperature: setting.temperature,
                    max_tokens: setting.max_tokens,
                });
            }
        }
    }

    Ok(ResolvedModel { model_id: (info.env_fallback)(config).to_string(), fallback_model_id: None, temperature: None, max_tokens: None })
}

/// The Vertex/OpenRouter ceiling lookup (`ai_provider::resolve_max_tokens`)
/// reads this directly rather than going through `resolve` — a caller
/// there already has a concrete `model_id` string (possibly one an
/// author picked via `POST .../generate-lesson`'s optional `model`
/// override, not necessarily the role's resolved default), so it needs
/// "what's this SPECIFIC model's ceiling", not "what model does this
/// role use".
pub async fn model_max_tokens(pool: &PgPool, model_id: &str) -> Result<Option<i64>, AppError> {
    let (catalog, _) = snapshot(pool).await?;
    Ok(catalog.iter().find(|c| c.model_id == model_id).and_then(|c| c.max_output_tokens).map(|v| v as i64))
}

/// Catalog entries with the `text` capability, enabled — what
/// `ai_provider::is_allowed_ai_model`/`GET /ai/models` (the Studio
/// author's model picker) offer. `vision`-only or `stt`/`tts`-only
/// catalog rows are never offered here: nothing in this codebase lets
/// an author freely pick an OCR or voice model per-request today.
pub async fn text_capable_models(pool: &PgPool) -> Result<Vec<CatalogEntry>, AppError> {
    let (catalog, _) = snapshot(pool).await?;
    Ok(catalog.into_iter().filter(|c| c.enabled && c.capabilities.iter().any(|cap| cap == "text")).collect())
}

fn validation_error(code: &'static str, detail: impl Into<String>) -> AppError {
    AppError::UnprocessableEntity(code, detail.into())
}

/// Checked before ANY catalog-referencing write (a role's `model_id`/
/// `fallback_model_id`, or re-enabling a catalog row) — the model must
/// exist, be enabled, and carry every capability the given role needs.
/// `field` names which JSON field a failure belongs to, so a caller
/// with two model ids to check (primary + fallback) can tell them apart
/// in the error.
async fn validate_model_for_role(pool: &PgPool, role: &AiRoleInfo, model_id: &str, field: &'static str) -> Result<(), AppError> {
    let (catalog, _) = snapshot(pool).await?;
    let Some(entry) = catalog.iter().find(|c| c.model_id == model_id) else {
        return Err(validation_error("model_not_in_catalog", format!(r#"{field}: model "{model_id}" tidak ada di katalog"#)));
    };
    if !entry.enabled {
        return Err(validation_error("model_disabled", format!(r#"{field}: model "{model_id}" sedang dinonaktifkan di katalog"#)));
    }
    let missing: Vec<&str> = role.required_capabilities.iter().filter(|cap| !entry.capabilities.iter().any(|c| c == *cap)).copied().collect();
    if !missing.is_empty() {
        return Err(validation_error(
            "model_missing_capability",
            format!(r#"{field}: model "{model_id}" tidak punya kemampuan yang dibutuhkan peran "{}": {}"#, role.id, missing.join(", ")),
        ));
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
pub struct SaveRoleInput {
    pub model_id: Option<String>,
    pub fallback_model_id: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i32>,
    pub daily_token_budget: Option<i32>,
    pub enabled: bool,
    /// ADR-0014 §1 — every admin action's audit row carries "alasannya".
    #[serde(default)]
    pub reason: Option<String>,
}

/// Full-replace (like `PATCH /module-items/{id}/quiz-config` — no
/// partial-merge trap): every field the caller doesn't set explicitly
/// clears to null/default. `model_id`/`fallback_model_id` of `None` is
/// a legitimate, meaningful value — "go back to the env fallback" — so
/// this never treats "not provided" as "leave whatever was there".
/// Exclusively an admin-write action (unlike `resolve`/`catalog_list`/
/// `role_list`, which every AI-calling request also reads) — the
/// `AdminPusat::Manage` gate lives right here, not split into a
/// handler-level check a future caller could forget.
pub async fn save_role(pool: &PgPool, ctx: &AuthContext, role_id: &str, input: SaveRoleInput) -> Result<RoleSettingRow, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::Manage)?;
    let actor_id = Some(ctx.user_id);
    let role = find_role(role_id).ok_or_else(|| AppError::NotFound("unknown_ai_role"))?;

    if let Some(model_id) = &input.model_id {
        validate_model_for_role(pool, role, model_id, "model_id").await?;
    }
    if let Some(fallback_id) = &input.fallback_model_id {
        validate_model_for_role(pool, role, fallback_id, "fallback_model_id").await?;
    }

    let before = role_list(pool).await?.into_iter().find(|r| r.role == role_id);

    sqlx::query!(
        r#"insert into ai_role_settings (role, model_id, fallback_model_id, temperature, max_tokens, daily_token_budget, enabled, updated_by, updated_at)
           values ($1, $2, $3, $4, $5, $6, $7, $8, now())
           on conflict (role) do update set
             model_id = excluded.model_id, fallback_model_id = excluded.fallback_model_id, temperature = excluded.temperature,
             max_tokens = excluded.max_tokens, daily_token_budget = excluded.daily_token_budget, enabled = excluded.enabled,
             updated_by = excluded.updated_by, updated_at = now()"#,
        role_id,
        input.model_id,
        input.fallback_model_id,
        input.temperature,
        input.max_tokens,
        input.daily_token_budget,
        input.enabled,
        actor_id,
    )
    .execute(pool)
    .await?;

    invalidate_cache(pool).await;
    let after = role_list(pool).await?.into_iter().find(|r| r.role == role_id).expect("just wrote this row");

    crate::services::admin_audit::record(
        pool,
        actor_id,
        "ai_role_settings.updated",
        "ai_role_settings",
        None,
        before.map(|b| serde_json::to_value(b).expect("RoleSettingRow serializes")),
        Some(serde_json::to_value(&after).expect("RoleSettingRow serializes")),
        input.reason.as_deref(),
    )
    .await?;

    Ok(after)
}

#[derive(Debug, serde::Serialize)]
pub struct RoleTestResult {
    pub model_id: String,
    pub latency_ms: u64,
    pub tokens_used: Option<i64>,
    pub sample_output: String,
}

/// `POST /admin/ai/roles/{role}/test` — a small real prompt through the
/// role's CURRENTLY RESOLVED model (whatever `resolve` returns right
/// now, override or fallback), so an admin can confirm a model actually
/// works before relying on it. Text-only: `stt`/`tts` roles need a
/// genuinely different test shape (audio in/out) this endpoint doesn't
/// attempt — out of scope for this ticket, returns a clear error
/// instead of pretending to test them.
pub async fn test_role(pool: &PgPool, config: &Config, ctx: &AuthContext, ai: &dyn crate::services::ai_provider::AIProvider, role_id: &str) -> Result<RoleTestResult, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::Manage)?;
    let role = find_role(role_id).ok_or(AppError::NotFound("unknown_ai_role"))?;
    if !role.required_capabilities.contains(&"text") {
        return Err(validation_error("role_not_text_testable", format!(r#"peran "{role_id}" tidak mendukung uji prompt teks (butuh kemampuan suara, bukan teks)"#)));
    }

    let resolved = resolve(pool, config, role_id).await?;
    // 128 was found live to be too small: a reasoning-capable model
    // (Vertex Gemini) can spend its ENTIRE budget on hidden
    // chain-of-thought before writing any visible reply, hitting
    // MAX_TOKENS with nothing to show — the exact class of bug
    // `resolve_max_tokens`'s own doc comment describes for real
    // generation calls. This test prompt gets the same generous
    // treatment as `ai_writing_evaluation`/`ai_speaking_evaluation`'s
    // short replies, capped to whatever the model's own catalog ceiling
    // allows.
    let max_tokens = crate::services::ai_provider::resolve_max_tokens(pool, &resolved.model_id, 1024).await;
    let start = std::time::Instant::now();
    let response = ai
        .generate(crate::services::ai_provider::GenerationRequest {
            model: resolved.model_id.clone(),
            system_prompt: "Anda sedang diuji dari halaman Pengaturan AI di Admin Pusat.".to_string(),
            user_prompt: "Balas singkat dalam satu kalimat Bahasa Indonesia untuk mengonfirmasi model ini berfungsi.".to_string(),
            temperature: 0.0,
            max_tokens,
            image_url: None,
            json_mode: false,
        })
        .await
        .map_err(|e| validation_error("ai_test_failed", format!("uji model gagal: {e}")))?;
    let latency_ms = start.elapsed().as_millis() as u64;

    Ok(RoleTestResult { model_id: resolved.model_id, latency_ms, tokens_used: response.tokens_used, sample_output: response.text })
}

#[derive(Debug, serde::Deserialize)]
pub struct PatchCatalogInput {
    pub enabled: Option<bool>,
    pub price_input_per_mtok_idr: Option<i32>,
    pub price_output_per_mtok_idr: Option<i32>,
    #[serde(default)]
    pub reason: Option<String>,
}

pub async fn patch_catalog_entry(pool: &PgPool, ctx: &AuthContext, id: Uuid, input: PatchCatalogInput) -> Result<CatalogEntry, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::Manage)?;
    let actor_id = Some(ctx.user_id);
    let before = catalog_list(pool).await?.into_iter().find(|c| c.id == id).ok_or(AppError::NotFound("ai_model_not_found"))?;

    sqlx::query!(
        r#"update ai_model_catalog set
             enabled = coalesce($2, enabled),
             price_input_per_mtok_idr = coalesce($3, price_input_per_mtok_idr),
             price_output_per_mtok_idr = coalesce($4, price_output_per_mtok_idr)
           where id = $1"#,
        id,
        input.enabled,
        input.price_input_per_mtok_idr,
        input.price_output_per_mtok_idr,
    )
    .execute(pool)
    .await?;

    invalidate_cache(pool).await;
    let after = catalog_list(pool).await?.into_iter().find(|c| c.id == id).expect("just wrote this row");

    crate::services::admin_audit::record(
        pool,
        actor_id,
        "ai_model_catalog.updated",
        "ai_model_catalog",
        Some(id),
        Some(serde_json::to_value(&before).expect("CatalogEntry serializes")),
        Some(serde_json::to_value(&after).expect("CatalogEntry serializes")),
        input.reason.as_deref(),
    )
    .await?;

    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_id_is_unique() {
        let mut ids: Vec<&str> = AI_ROLES.iter().map(|r| r.id).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before, "duplicate role id in AI_ROLES");
    }

    #[test]
    fn find_role_looks_up_by_id() {
        assert!(find_role("lesson_generation").is_some());
        assert!(find_role("not_a_real_role").is_none());
    }

    #[test]
    fn quiz_generation_and_lesson_generation_share_the_same_fallback_field_today() {
        // Codifies the deliberate design note in AI_ROLES's doc comment
        // — if this ever diverges it must be a conscious change, not a
        // silent copy-paste drift.
        let config = crate::config::Config {
            bind_addr: "0.0.0.0:0".into(),
            database_url: String::new(),
            jwt_access_secret: "x".into(),
            access_token_ttl_minutes: 15,
            refresh_token_ttl_days: 30,
            frontend_origin: "x".into(),
            google_client_id: "x".into(),
            mastery_confidence_threshold: 0.6,
            weakness_score_threshold: 60.0,
            rescue_mode_consecutive_failures: 3,
            review_queue_default_limit: 10,
            review_queue_min_gap_hours: 4,
            attendance_min_duration_ratio: 0.75,
            attendance_late_join_minutes: 10,
            ai_stt_model: "stt-model".into(),
            ai_tts_default_voice: "voice".into(),
            ai_speaking_room_text_model: "x".into(),
            ai_speaking_room_tts_model: "x".into(),
            asset_max_bytes: 1,
            asset_signed_url_ttl_seconds: 1,
            asset_presigned_put_ttl_seconds: 1,
            asset_public_signed_url_ttl_seconds: 1,
            mastery_lambda: 0.05,
            mastery_n_min: 5.0,
            frss_recalled_threshold: 0.8,
            frss_partial_threshold: 0.4,
            module_completion_min_accuracy: 80.0,
            module_completion_skip_credit_cost: 15,
            ai_writing_evaluation_model: "x".into(),
            ai_speaking_evaluation_model: "x".into(),
            ai_grammar_evaluation_credit_cost: 1,
            ai_grammar_evaluation_model: "x".into(),
            ai_lesson_generation_model: "the-lesson-model".into(),
            ai_question_generation_model: "x".into(),
            ai_ocr_model: "x".into(),
            ai_tts_model: "x".into(),
            redis_url: "x".into(),
            collab_checkpoint_interval_seconds: 1,
            ai_live_chat_model: "x".into(),
            gcp_project_id: "x".into(),
            gcp_region: "x".into(),
            consent_guardian_confirmation_required: false,
        };
        let lesson = find_role("lesson_generation").unwrap();
        let quiz = find_role("quiz_generation").unwrap();
        assert_eq!((lesson.env_fallback)(&config), (quiz.env_fallback)(&config));
        assert_eq!((quiz.env_fallback)(&config), "the-lesson-model");
    }
}
