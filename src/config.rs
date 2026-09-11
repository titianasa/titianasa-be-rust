use anyhow::Context;

// Port of titian-backend-bun/src/config.ts's `loadConfig()`. Only the
// fields R0+R1 (scaffold + auth/users) need exist so far — the rest of
// the ~65-field Bun Config grows onto this struct incrementally as each
// later migration phase (see agent/docs/tickets/phase-30-rust-migration.md)
// is ported, same as the Bun one grew across 29 phases.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub redis_url: String,
    pub jwt_access_secret: String,
    pub access_token_ttl_minutes: i64,
    pub refresh_token_ttl_days: i64,
    pub frontend_origin: String,
    pub google_client_id: String,
    // R5 (mastery/FRSS/rescue) additions.
    pub mastery_confidence_threshold: f64,
    pub weakness_score_threshold: f64,
    pub rescue_mode_consecutive_failures: i64,
    pub review_queue_default_limit: i64,
    pub review_queue_min_gap_hours: i64,
    // R8 (class sessions / attendance verification / Speaking Room) additions.
    pub attendance_min_duration_ratio: f64,
    pub attendance_late_join_minutes: i64,
    pub ai_stt_model: String,
    pub ai_tts_default_voice: String,
    pub ai_speaking_room_text_model: String,
    pub ai_speaking_room_tts_model: String,
    // R9 (Drive/assets) additions.
    pub asset_max_bytes: i64,
    pub asset_signed_url_ttl_seconds: i64,
    pub asset_presigned_put_ttl_seconds: i64,
    pub asset_public_signed_url_ttl_seconds: i64,
    // R12 (attempt submission/grading + AI content generation) additions.
    pub mastery_lambda: f64,
    pub mastery_n_min: f64,
    pub frss_recalled_threshold: f64,
    pub frss_partial_threshold: f64,
    pub module_completion_min_accuracy: f64,
    pub module_completion_skip_credit_cost: i64,
    pub ai_writing_evaluation_model: String,
    pub ai_speaking_evaluation_model: String,
    pub ai_grammar_evaluation_credit_cost: i64,
    pub ai_grammar_evaluation_model: String,
    pub ai_lesson_generation_model: String,
    pub ai_question_generation_model: String,
    pub ai_ocr_model: String,
    pub ai_tts_model: String,
    // Phase 31 (P31-005) — real-time collaborative editing.
    pub collab_checkpoint_interval_seconds: i64,
    // Phase 37 — Live AI Chat (the Modul Belajar text tutor).
    pub ai_live_chat_model: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            bind_addr: std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8090".into()),
            database_url: std::env::var("DATABASE_URL").context("DATABASE_URL is required")?,
            redis_url: std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into()),
            jwt_access_secret: std::env::var("JWT_ACCESS_SECRET")
                .context("JWT_ACCESS_SECRET is required")?,
            access_token_ttl_minutes: env_int("ACCESS_TOKEN_TTL_MINUTES", 15)?,
            refresh_token_ttl_days: env_int("REFRESH_TOKEN_TTL_DAYS", 30)?,
            frontend_origin: std::env::var("FRONTEND_ORIGIN")
                .unwrap_or_else(|_| "http://localhost:3000".into()),
            google_client_id: std::env::var("GOOGLE_CLIENT_ID")
                .context("GOOGLE_CLIENT_ID is required")?,
            mastery_confidence_threshold: env_float("MASTERY_CONFIDENCE_THRESHOLD", 0.6)?,
            weakness_score_threshold: env_float("WEAKNESS_SCORE_THRESHOLD", 60.0)?,
            rescue_mode_consecutive_failures: env_int("RESCUE_MODE_CONSECUTIVE_FAILURES", 3)?,
            review_queue_default_limit: env_int("REVIEW_QUEUE_DEFAULT_LIMIT", 10)?,
            review_queue_min_gap_hours: env_int("REVIEW_QUEUE_MIN_GAP_HOURS", 4)?,
            attendance_min_duration_ratio: env_float("ATTENDANCE_MIN_DURATION_RATIO", 0.75)?,
            attendance_late_join_minutes: env_int("ATTENDANCE_LATE_JOIN_MINUTES", 10)?,
            ai_stt_model: std::env::var("AI_STT_MODEL").unwrap_or_else(|_| "openai/whisper-1".into()),
            ai_tts_default_voice: std::env::var("AI_TTS_DEFAULT_VOICE").unwrap_or_else(|_| "af_bella".into()),
            ai_speaking_room_text_model: std::env::var("AI_SPEAKING_ROOM_TEXT_MODEL")
                .unwrap_or_else(|_| "deepseek/deepseek-v4.1-flash".into()),
            ai_speaking_room_tts_model: std::env::var("AI_SPEAKING_ROOM_TTS_MODEL")
                .unwrap_or_else(|_| "google/gemini-3.1-flash-tts-preview".into()),
            asset_max_bytes: env_int("ASSET_MAX_BYTES", 25 * 1024 * 1024)?,
            asset_signed_url_ttl_seconds: env_int("ASSET_SIGNED_URL_TTL_SECONDS", 3600)?,
            asset_presigned_put_ttl_seconds: env_int("ASSET_PRESIGNED_PUT_TTL_SECONDS", 900)?,
            asset_public_signed_url_ttl_seconds: env_int("ASSET_PUBLIC_SIGNED_URL_TTL_SECONDS", 604_800)?,
            mastery_lambda: env_float("MASTERY_LAMBDA", 0.05)?,
            mastery_n_min: env_float("MASTERY_N_MIN", 5.0)?,
            frss_recalled_threshold: env_float("FRSS_RECALLED_THRESHOLD", 0.8)?,
            frss_partial_threshold: env_float("FRSS_PARTIAL_THRESHOLD", 0.4)?,
            module_completion_min_accuracy: env_float("MODULE_COMPLETION_MIN_ACCURACY", 80.0)?,
            module_completion_skip_credit_cost: env_int("MODULE_COMPLETION_SKIP_CREDIT_COST", 15)?,
            ai_writing_evaluation_model: std::env::var("AI_WRITING_EVALUATION_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4.1-flash".into()),
            ai_speaking_evaluation_model: std::env::var("AI_SPEAKING_EVALUATION_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4.1-flash".into()),
            ai_grammar_evaluation_credit_cost: env_int("AI_GRAMMAR_EVALUATION_CREDIT_COST", 1)?,
            ai_grammar_evaluation_model: std::env::var("AI_GRAMMAR_EVALUATION_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4.1-flash".into()),
            ai_lesson_generation_model: std::env::var("AI_LESSON_GENERATION_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4.1-flash".into()),
            ai_question_generation_model: std::env::var("AI_QUESTION_GENERATION_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4.1-flash".into()),
            ai_ocr_model: std::env::var("AI_OCR_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4-flash-vision-exp".into()),
            ai_tts_model: std::env::var("AI_TTS_MODEL").unwrap_or_else(|_| "hexgrad/kokoro-82m".into()),
            collab_checkpoint_interval_seconds: env_int("COLLAB_CHECKPOINT_INTERVAL_SECONDS", 15)?,
            ai_live_chat_model: std::env::var("AI_LIVE_CHAT_MODEL").unwrap_or_else(|_| "deepseek/deepseek-v4.1-flash".into()),
        })
    }
}

fn env_int(key: &str, default: i64) -> anyhow::Result<i64> {
    match std::env::var(key) {
        Ok(v) => v.parse().with_context(|| format!("{key} must be a valid integer")),
        Err(_) => Ok(default),
    }
}

fn env_float(key: &str, default: f64) -> anyhow::Result<f64> {
    match std::env::var(key) {
        Ok(v) => v.parse().with_context(|| format!("{key} must be a valid number")),
        Err(_) => Ok(default),
    }
}
