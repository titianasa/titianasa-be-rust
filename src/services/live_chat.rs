// Live AI Chat — the text-based tutor mode of a Modul Belajar, ported
// from parelabs' LiveClassChatRoom. Deliberately stateless like
// speaking_room.rs: no DB persistence of the conversation itself, the
// client owns it and resends it each turn — but each turn still spends
// real tokens, so it gets an `ai_tasks` row for usage audit like every
// other generator does (Phase 38; this used to be the one exception).
//
// Voice (STT input / TTS output) is intentionally NOT ported yet — see
// the phase-37 ticket. This is text in, text out.

use std::sync::Arc;

use axum::response::sse::Event;
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::{resolve_max_tokens, AIProvider, GenerationRequest};
use crate::services::ai_task;
use crate::services::lesson_plan::{self, LessonPlan};
use crate::services::lesson_plan_ai::language_name;
use crate::services::module_item;

const PROVIDER: &str = "openrouter";
const PROMPT_ID: &str = "live_chat_turn_v1";

/// How many of the trailing history entries actually go to the model —
/// unlike speaking_room's single-exchange turns, a tutoring conversation
/// needs enough context to remember what was already covered.
const MAX_HISTORY_TURNS: usize = 20;
const MAX_MESSAGE_CHARS: usize = 4000;
/// P39-006 — the question TEXT stored in `learning_events` is capped
/// shorter than the model actually receives (`MAX_MESSAGE_CHARS`); this
/// is a record for later review/analytics, not the AI's own context.
const MAX_STORED_QUESTION_CHARS: usize = 500;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatHistoryEntry {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatTurnRequest {
    pub item_id: Uuid,
    /// The plan as the learner is currently reading it — already
    /// translated if they picked a different language than it was
    /// authored in, so the tutor teaches in the same words they see.
    pub lesson_plan: serde_json::Value,
    pub section_index: usize,
    pub language: String,
    pub message: String,
    #[serde(default)]
    pub history: Vec<ChatHistoryEntry>,
}

#[derive(Debug, Serialize)]
pub struct ChatTurnResponse {
    pub reply: String,
}

fn validation_error(code: &'static str, detail: impl Into<String>) -> AppError {
    AppError::UnprocessableEntity(code, detail.into())
}

fn outline(plan: &LessonPlan) -> String {
    plan.sections
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let title = if s.title.is_empty() { "(tanpa judul)".to_string() } else { s.title.clone() };
            let goal = if s.goal.is_empty() { String::new() } else { format!(" — tujuan: {}", s.goal) };
            format!("  {}. {title}{goal}", i + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn excerpt(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(limit).collect::<String>())
    }
}

pub fn system_prompt(plan: &LessonPlan, section_index: usize, language: &str) -> String {
    let section = &plan.sections[section_index];
    let title = if plan.title.is_empty() { "Modul Belajar".to_string() } else { plan.title.clone() };
    let level = if plan.level.is_empty() { String::new() } else { format!(" (tingkat {})", plan.level) };
    let section_title = if section.title.is_empty() { "(tanpa judul)".to_string() } else { section.title.clone() };
    let goal_line = if section.goal.is_empty() { String::new() } else { format!("\nTujuan bagian ini: {}", section.goal) };
    let outline_text = outline(plan);

    format!(
        "Anda adalah Tutor AI — tutor yang ramah, sabar, dan komunikatif, sedang memandu kelas langsung satu-lawan-satu untuk modul \"{title}\"{level}.\n\n\
BAHASA PENGAJARAN: {language}. WAJIB membalas dalam {language} di SETIAP giliran, apa pun bahasa pesan siswa. Pengecualian: pertahankan contoh berbahasa target, istilah teknis, rumus, dan kutipan kode apa adanya.\n\n\
Garis besar modul ({total} bagian):\n{outline}\n\n\
Sedang mengajar: BAGIAN {num} dari {total} — \"{section_title}\"{goal_line}\n\n\
Materi bagian ini:\n\"\"\"\n{content}\n\"\"\"\n\n\
CARA MENGAJAR\n\
- Sapa hangat dan sebutkan topiknya HANYA di awal percakapan (riwayat kosong) — jangan mengulang sapaan di balasan berikutnya.\n\
- Jelaskan satu konsep dari materi, beri satu contoh konkret, lalu ajukan SATU pertanyaan spesifik yang bisa langsung dijawab siswa. Jangan mengajukan pertanyaan mengambang seperti \"ada yang ingin ditanyakan?\".\n\
- Jawaban siswa salah → koreksi singkat (yang salah → yang benar → kenapa), lalu tanya ulang dengan cara berbeda atau lanjut ke poin berikutnya.\n\
- Jawaban siswa benar → apresiasi singkat (\"Betul!\", \"Tepat sekali!\"), lalu lanjut.\n\
- Boleh pakai **tebal** untuk istilah kunci, daftar bernomor/bullet, dan `> kutipan` saat mengutip jawaban siswa — akan dirender sebagai markdown.\n\
- Setelah beberapa giliran yang membahas tujuan bagian ini, TAWARKAN pindah ke bagian berikutnya; jangan berpindah sendiri tanpa persetujuan siswa.\n\
- Balasan ringkas: 2-5 kalimat pendek per giliran. Jangan menulis esai panjang atau mengajar lebih dari itu sebelum bertanya lagi.\n\
- JANGAN membuat soal ujian formal (pilihan ganda, isian, dsb) — itu tugas item Kuis terpisah. Ini sesi diskusi dan tanya-jawab.",
        outline = outline_text,
        total = plan.sections.len(),
        num = section_index + 1,
        content = section.content,
    )
}

pub fn user_prompt(message: &str, history: &[ChatHistoryEntry]) -> String {
    let start = history.len().saturating_sub(MAX_HISTORY_TURNS);
    let context = history[start..]
        .iter()
        .map(|m| format!("{}: {}", if m.role == "user" { "Siswa" } else { "Tutor" }, excerpt(&m.content, 2000)))
        .collect::<Vec<_>>()
        .join("\n");
    let context = if context.is_empty() { "Awal percakapan.".to_string() } else { context };
    format!("Riwayat percakapan sejauh ini:\n{context}\n\nPesan terbaru siswa:\n\"{message}\"\n\nBalas sebagai Tutor AI, ikuti aturan di atas.")
}

/// P39-006 — one `live_chat_question` event per student turn, whether
/// or not the tutor's reply ever arrives (a provider failure shouldn't
/// erase that the student DID ask something). Gated on `ai_chat_storage`
/// consent inside `learning_event::record` itself — this function never
/// branches on consent, it just calls through and ignores the `None`
/// (chat still works without it, per ADR-0013 §1.4).
async fn record_question_event(pool: &PgPool, user_id: Uuid, item_id: Uuid, plan: &LessonPlan, section_index: usize, message: &str) {
    let current_version: Option<i32> = sqlx::query_scalar!(r#"select current_version from module_items where id = $1"#, item_id).fetch_optional(pool).await.ok().flatten().flatten();
    let mut event = crate::services::learning_event::NewLearningEvent::server(
        "live_chat_question",
        "module_item",
        item_id,
        serde_json::json!({"question": excerpt(message, MAX_STORED_QUESTION_CHARS)}),
        "live_ai_chat",
    );
    event.module_item_id = Some(item_id);
    event.content_uid = Some(plan.sections[section_index].id.clone());
    event.content_version = current_version;
    if let Err(e) = crate::services::learning_event::record(pool, user_id, crate::services::learning_event::EventChannel::Server, event).await {
        tracing::warn!(error = ?e, %item_id, "failed to record live_chat_question event");
    }
}

pub async fn generate_turn(pool: &PgPool, ai: &dyn AIProvider, model: &str, ctx: &AuthContext, req: ChatTurnRequest) -> Result<ChatTurnResponse, AppError> {
    // Same read gate as opening the item — a locked or unpublished
    // Modul Belajar has nothing here either.
    let item = module_item::get_detail(pool, ctx, req.item_id).await?;
    if item.content_type.as_deref() != Some("article") {
        return Err(validation_error("not_an_article", "obrolan AI hanya tersedia untuk Modul Belajar"));
    }

    let message = req.message.trim();
    if message.is_empty() {
        return Err(validation_error("message_required", "pesan tidak boleh kosong"));
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(validation_error("message_too_long", format!("pesan maksimal {MAX_MESSAGE_CHARS} karakter")));
    }

    let language_label = language_name(&req.language).ok_or_else(|| validation_error("invalid_language", "bahasa tidak dikenal"))?;
    let plan = lesson_plan::parse(&req.lesson_plan)?;
    if plan.sections.is_empty() {
        return Err(validation_error("empty_plan", "modul ini belum punya bagian untuk didiskusikan"));
    }
    let section_index = req.section_index.min(plan.sections.len() - 1);

    record_question_event(pool, ctx.user_id, req.item_id, &plan, section_index, message).await;

    let ai_task_id = Uuid::new_v4();
    let max_tokens = resolve_max_tokens(model, 900).await;
    let request = GenerationRequest {
        model: model.to_string(),
        system_prompt: system_prompt(&plan, section_index, language_label),
        user_prompt: user_prompt(message, &req.history),
        temperature: 0.7,
        max_tokens,
        image_url: None,
        json_mode: false,
    };

    // One retry on the SAME provider — a transient hiccup is worth one
    // more try before the author sees a failure at all; never falls back
    // to a different provider or model.
    let mut outcome = None;
    let mut last_error = String::new();
    for attempt in 0..2 {
        match ai.generate(request.clone()).await {
            Ok(g) if !g.text.trim().is_empty() => {
                outcome = Some(g);
                break;
            }
            Ok(_) => {
                tracing::warn!(%ai_task_id, attempt, item_id = %req.item_id, "live chat turn generation returned an empty reply");
                last_error = "Model mengembalikan balasan kosong.".to_string();
            }
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, item_id = %req.item_id, "live chat turn generation failed");
                last_error = format!("Provider gagal: {e}");
            }
        }
    }
    let Some(generation) = outcome else {
        let _ = ai_task::insert_failed(pool, ai_task_id, ctx.user_id, "live_chat_turn", PROVIDER, model, PROMPT_ID).await;
        return Err(AppError::AiOutputValidationFailed(Some(last_error)));
    };

    let reply = generation.text.trim();
    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "live_chat_turn", PROVIDER, model, PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;
    Ok(ChatTurnResponse { reply: reply.to_string() })
}

/// The streaming twin of `generate_turn` — same validation, same prompt,
/// but forwards each text delta to the client as it arrives instead of
/// waiting for the whole reply. No retry: once even one chunk has
/// reached the client there's no way to invisibly discard it and start
/// over, so a mid-stream provider failure just ends the stream early
/// (the learner keeps whatever partial reply already rendered) — only a
/// failure to open the stream AT ALL is a normal request error, same
/// shape as `generate_turn`'s.
pub async fn generate_turn_stream(
    pool: PgPool,
    ai: Arc<dyn AIProvider>,
    model: String,
    ctx: AuthContext,
    req: ChatTurnRequest,
) -> Result<impl Stream<Item = Result<Event, std::convert::Infallible>>, AppError> {
    let item = module_item::get_detail(&pool, &ctx, req.item_id).await?;
    if item.content_type.as_deref() != Some("article") {
        return Err(validation_error("not_an_article", "obrolan AI hanya tersedia untuk Modul Belajar"));
    }

    let message = req.message.trim().to_string();
    if message.is_empty() {
        return Err(validation_error("message_required", "pesan tidak boleh kosong"));
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(validation_error("message_too_long", format!("pesan maksimal {MAX_MESSAGE_CHARS} karakter")));
    }

    let language_label = language_name(&req.language).ok_or_else(|| validation_error("invalid_language", "bahasa tidak dikenal"))?;
    let plan = lesson_plan::parse(&req.lesson_plan)?;
    if plan.sections.is_empty() {
        return Err(validation_error("empty_plan", "modul ini belum punya bagian untuk didiskusikan"));
    }
    let section_index = req.section_index.min(plan.sections.len() - 1);

    record_question_event(&pool, ctx.user_id, req.item_id, &plan, section_index, &message).await;

    let ai_task_id = Uuid::new_v4();
    let max_tokens = resolve_max_tokens(&model, 900).await;
    let request = GenerationRequest {
        model: model.clone(),
        system_prompt: system_prompt(&plan, section_index, language_label),
        user_prompt: user_prompt(&message, &req.history),
        temperature: 0.7,
        max_tokens,
        image_url: None,
        json_mode: false,
    };

    let inner = ai.generate_stream(request).await.map_err(|e| {
        tracing::warn!(error = ?e, %ai_task_id, item_id = %req.item_id, "live chat turn stream failed to open");
        AppError::AiOutputValidationFailed(Some(format!("Provider gagal: {e}")))
    })?;

    struct State {
        inner: crate::services::ai_provider::TextChunkStream,
        pool: PgPool,
        user_id: Uuid,
        item_id: Uuid,
        ai_task_id: Uuid,
        model: String,
        accumulated: String,
        done: bool,
    }
    let state = State { inner, pool, user_id: ctx.user_id, item_id: req.item_id, ai_task_id, model, accumulated: String::new(), done: false };

    let stream = futures_util::stream::unfold(state, |mut st| async move {
        if st.done {
            return None;
        }
        match st.inner.next().await {
            Some(Ok(chunk)) => {
                st.accumulated.push_str(&chunk);
                Some((Ok(Event::default().event("chunk").data(chunk)), st))
            }
            Some(Err(e)) => {
                let ai_task_id = st.ai_task_id;
                let item_id = st.item_id;
                tracing::warn!(error = ?e, %ai_task_id, %item_id, "live chat turn stream failed mid-reply");
                st.done = true;
                let reply = st.accumulated.trim();
                if reply.is_empty() {
                    let _ = ai_task::insert_failed(&st.pool, st.ai_task_id, st.user_id, "live_chat_turn", PROVIDER, &st.model, PROMPT_ID).await;
                } else {
                    // A real (if truncated) reply already reached the
                    // learner — that's a done turn, not a failed one.
                    let _ = ai_task::insert_done(&st.pool, st.ai_task_id, st.user_id, "live_chat_turn", PROVIDER, &st.model, PROMPT_ID, None).await;
                }
                Some((Ok(Event::default().event("error").data(e.0.clone())), st))
            }
            None => {
                st.done = true;
                let reply = st.accumulated.trim().to_string();
                if reply.is_empty() {
                    let _ = ai_task::insert_failed(&st.pool, st.ai_task_id, st.user_id, "live_chat_turn", PROVIDER, &st.model, PROMPT_ID).await;
                    return Some((Ok(Event::default().event("error").data("Model mengembalikan balasan kosong.")), st));
                }
                let _ = ai_task::insert_done(&st.pool, st.ai_task_id, st.user_id, "live_chat_turn", PROVIDER, &st.model, PROMPT_ID, None).await;
                Some((Ok(Event::default().event("done").data("")), st))
            }
        }
    });

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> LessonPlan {
        lesson_plan::normalize(LessonPlan {
            title: "Gerak Lurus".to_string(),
            topic: "Kinematika".to_string(),
            level: "SMA".to_string(),
            language: "id".to_string(),
            sections: vec![
                lesson_plan::LessonPlanSection { id: "a".into(), title: "Pengantar".into(), minutes: Some(5), goal: "Paham definisi".into(), content: "Gerak lurus adalah...".into() },
                lesson_plan::LessonPlanSection { id: "b".into(), title: "Rumus".into(), minutes: Some(5), goal: String::new(), content: "v = s/t".into() },
            ],
        })
        .unwrap()
    }

    #[test]
    fn system_prompt_names_the_current_section_and_language() {
        let prompt = system_prompt(&plan(), 1, "English");
        assert!(prompt.contains("BAGIAN 2 dari 2"));
        assert!(prompt.contains("\"Rumus\""));
        assert!(prompt.contains("v = s/t"));
        assert!(prompt.contains("BAHASA PENGAJARAN: English"));
        // Section 2 has no goal — no dangling "tujuan bagian ini:" label.
        assert!(!prompt.contains("Tujuan bagian ini: \n"));
    }

    #[test]
    fn system_prompt_lists_every_section_in_the_outline() {
        let prompt = system_prompt(&plan(), 0, "Bahasa Indonesia");
        assert!(prompt.contains("1. Pengantar — tujuan: Paham definisi"));
        assert!(prompt.contains("2. Rumus"));
    }

    #[test]
    fn user_prompt_renders_history_in_order_and_trims_old_turns() {
        let history: Vec<ChatHistoryEntry> =
            (0..25).map(|i| ChatHistoryEntry { role: if i % 2 == 0 { "user".into() } else { "assistant".into() }, content: format!("turn {i}") }).collect();
        let prompt = user_prompt("pertanyaan baru", &history);
        assert!(!prompt.contains("turn 0"), "the oldest turns should be dropped");
        assert!(prompt.contains("turn 24"));
        assert!(prompt.contains("Siswa: turn 24"));
        assert!(prompt.contains("pertanyaan baru"));
        // "turn 24" appears before "turn 5" in the trimmed window's order.
        assert!(prompt.find("turn 5").unwrap() < prompt.find("turn 24").unwrap());
    }

    #[test]
    fn user_prompt_marks_an_empty_history_as_the_start() {
        assert!(user_prompt("halo", &[]).contains("Awal percakapan."));
    }

    #[test]
    fn out_of_range_section_index_clamps_instead_of_panicking() {
        // generate_turn clamps before calling system_prompt; verify the
        // clamp math directly since generate_turn itself needs a DB.
        let p = plan();
        let clamped = 99usize.min(p.sections.len() - 1);
        assert_eq!(clamped, 1);
    }
}
