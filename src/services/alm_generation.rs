// The ALM editor's right-click "AI Asisten" — generates a fragment of
// content to insert exactly at the author's cursor, ported from
// parelabs' AIInlineAssistant (same UX: right-click the canvas, type
// what to insert, the answer lands where the cursor was).
//
// Deliberately item-agnostic, unlike lesson_plan_ai.rs's other calls:
// AlmEditor is the ONE editor shared by plain articles, Modul Belajar
// sections, and every quiz field's AlmField modal — none of which
// always has an item id in scope (AlmField in particular has none).
// A plain role check is the gate, the same tier that already lets
// someone open any of those editors to type by hand.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::ai_task;
use crate::services::lesson_plan;
use crate::services::lesson_plan_ai::ALM_GUIDE;
use crate::services::permissions::{require_permission, Action, Resource};

const PROVIDER: &str = "openrouter";
const PROMPT_ID: &str = "alm_fragment_generation_v1";
/// Instruction text plus whatever reference material the author
/// attaches (a section excerpt, an extracted PDF) travel together in
/// one field — capped generously since a document attachment alone can
/// run tens of thousands of characters.
const MAX_INSTRUCTION_CHARS: usize = 80_000;

#[derive(Debug, serde::Deserialize)]
pub struct GenerateFragmentRequest {
    pub instruction: String,
}

#[derive(Debug, serde::Serialize)]
pub struct GenerateFragmentResponse {
    pub content: String,
}

fn system_prompt() -> String {
    format!(
        "Anda asisten penulisan yang menyisipkan konten PERSIS di posisi kursor penulis, di dalam sebuah dokumen ALM. Anda tidak tahu apa isi dokumen di sekitarnya — hanya kerjakan permintaan penulis.\n\n\
{ALM_GUIDE}\n\n\
ATURAN\n\
- Keluarkan HANYA konten ALM yang akan disisipkan — tanpa basa-basi pembuka/penutup, tanpa pagar kode yang membungkus seluruh keluaran.\n\
- Sisipkan PERSIS yang diminta — satu paragraf, satu tabel, satu blok gambar, satu daftar langkah, dst. Jangan menambahkan judul atau bagian baru yang tidak diminta.\n\
- Bila penulis melampirkan materi rujukan (kutipan bagian lain, isi dokumen), pakai itu sebagai ACUAN KEBENARAN — jangan menyalinnya mentah-mentah kecuali diminta mengutip.\n\
- Untuk media (gambar/video/audio), pakai HANYA alamat https yang benar-benar diberikan penulis — jangan pernah mengarang alamat.\n\
- Tulis dalam bahasa yang sama dengan permintaan penulis, kecuali diminta bahasa lain."
    )
}

/// A right-click assist with no ITEM to associate a row with (AlmField
/// in particular has none in scope) — but it still spends real tokens,
/// so it's still worth an `ai_tasks` row for usage audit (Phase 38),
/// same as every other generator; `ai_tasks` never required an item_id
/// to begin with.
pub async fn generate_fragment(pool: &PgPool, ai: &dyn AIProvider, model: &str, ctx: &AuthContext, req: GenerateFragmentRequest) -> Result<GenerateFragmentResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;

    let instruction = req.instruction.trim();
    if instruction.is_empty() {
        return Err(AppError::UnprocessableEntity("instruction_required", "instruksi wajib diisi".to_string()));
    }
    if instruction.chars().count() > MAX_INSTRUCTION_CHARS {
        return Err(AppError::UnprocessableEntity("instruction_too_long", format!("instruksi dan lampiran terlalu panjang (maks. {MAX_INSTRUCTION_CHARS} karakter)")));
    }

    let ai_task_id = Uuid::new_v4();
    let max_tokens = resolve_max_tokens(model, 3000).await;
    let request = GenerationRequest {
        model: model.to_string(),
        system_prompt: system_prompt(),
        user_prompt: instruction.to_string(),
        temperature: 0.6,
        max_tokens,
        image_url: None,
        json_mode: false,
    };

    // One retry on the SAME provider — never falls back to a different
    // one; sanitize_generated's own "the schema rejected everything"
    // case (empty content) also gets a second attempt, same as a
    // straight provider failure.
    let mut outcome = None;
    let mut last_error = String::new();
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, provider = PROVIDER, prompt_id = PROMPT_ID, "alm fragment generation failed");
                last_error = format!("Provider gagal: {e}");
                continue;
            }
        };
        // sanitize_generated guarantees parseable ALM back (demoting any
        // block the schema rejects to prose) rather than handing the
        // editor text it would refuse to load.
        let content = lesson_plan::sanitize_generated(&strip_code_fence(&generation.text));
        if content.trim().is_empty() {
            tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "alm fragment generation produced no usable content");
            last_error = "Model tidak mengembalikan konten yang bisa dipakai.".to_string();
            continue;
        }
        outcome = Some((content, generation.tokens_used));
        break;
    }
    let Some((content, tokens_used)) = outcome else {
        let _ = ai_task::insert_failed(pool, ai_task_id, ctx.user_id, "alm_fragment", PROVIDER, model, PROMPT_ID).await;
        return Err(AppError::AiOutputValidationFailed(Some(last_error)));
    };
    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "alm_fragment", PROVIDER, model, PROMPT_ID, tokens_used.map(|t| t as i32)).await?;
    Ok(GenerateFragmentResponse { content })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_carries_the_alm_dialect_and_the_cursor_framing() {
        let prompt = system_prompt();
        assert!(prompt.contains("posisi kursor"));
        assert!(prompt.contains(":::callout"));
        assert!(prompt.contains("Jangan menambahkan judul"));
    }
}
