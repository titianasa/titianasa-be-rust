// AI for the Modul Belajar editor, ported from parelabs:
//   generate_plan     ← /ai/live-class/generate (topic → whole plan)
//   edit_section      ← the per-section "Edit dengan AI" composer
//   translate_plan    ← /ai/live-class/translate (the learner's language)
//
// All three use a DELIMITED output protocol instead of JSON. A section
// body is long prose full of quotes, code fences, LaTeX backslashes and
// ALM directives, every one of which a model must escape correctly
// inside a JSON string — and one `\times` arriving as TAB+"imes"
// silently corrupts a formula. Only the short metadata (title, goal,
// minutes) travels as JSON.

use futures_util::future::join_all;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::ai::{EditLessonSectionRequest, GenerateLessonPlanRequest, TranslateLessonPlanRequest};
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::lesson_plan::{self, LessonPlan, LessonPlanSection};
use crate::services::{ai_task, module_item, quiz_config_schema};

const PROVIDER: &str = "openrouter";
const GENERATE_PROMPT_ID: &str = "lesson_plan_generation_v1";
const EDIT_PROMPT_ID: &str = "lesson_section_edit_v1";
const TRANSLATE_PROMPT_ID: &str = "lesson_plan_translation_v1";

const PLAN_OPEN: &str = "<<<PLAN>>>";
const PLAN_CLOSE: &str = "<<<END_PLAN>>>";
const SECTION_OPEN: &str = "<<<SECTION>>>";
const SECTION_CLOSE: &str = "<<<END_SECTION>>>";
const META_OPEN: &str = "<<<META>>>";
const META_CLOSE: &str = "<<<END_META>>>";
const CONTENT_OPEN: &str = "<<<CONTENT>>>";
const CONTENT_CLOSE: &str = "<<<END_CONTENT>>>";

/// Languages a plan can be written in and a learner can read it in —
/// the same 13 parelabs offers on both sides.
pub const LANGUAGES: [(&str, &str); 13] = [
    ("id", "Bahasa Indonesia"),
    ("en", "English"),
    ("de", "Deutsch (German)"),
    ("ar", "العربية (Arabic)"),
    ("zh", "中文 (Mandarin Chinese)"),
    ("ja", "日本語 (Japanese)"),
    ("ko", "한국어 (Korean)"),
    ("fr", "Français (French)"),
    ("jv", "Basa Jawa (Javanese)"),
    ("es", "Español (Spanish)"),
    ("pt", "Português (Portuguese)"),
    ("nl", "Nederlands (Dutch)"),
    ("ru", "Русский (Russian)"),
];

pub fn language_name(code: &str) -> Option<&'static str> {
    LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, name)| *name)
}

/// What the model may write inside a section. Spelled out block by
/// block with exact keys, because block_schema rejects anything else —
/// and a vague "you may use callouts" produced invented keys every time.
// pub(crate): also used by alm_generation.rs's right-click "AI Asisten"
// (same ALM dialect, so the two never drift into describing it differently).
pub(crate) const ALM_GUIDE: &str = r####"FORMAT ISI BAGIAN — ALR Learning Markdown (ALM)
Ini BUKAN markdown bebas. Blok yang tidak dikenal atau memakai kunci yang tidak tercantum akan ditolak.

TEKS
- Paragraf: teks biasa. Pisahkan paragraf dengan satu baris kosong.
- Format dalam kalimat: **tebal**, *miring*, ~~coret~~, `kode`, [teks tautan](https://alamat).
- Subjudul: "## " atau "### ". Jangan memakai "# ", dan jangan mengulang judul bagian sebagai subjudul pertama.
- Daftar: "- butir" atau "1. butir", satu butir per baris.
- Contoh kalimat/ungkapan satu baris: "> contoh".
- Kode: pagar ``` dengan nama bahasanya, ditutup ```.
- Rumus: LaTeX di dalam teks — $...$ sebaris, $$...$$ di baris sendiri.

BLOK KHUSUS
Ditulis sebagai baris ":::nama", lalu baris-baris "kunci: nilai", lalu baris ":::". Setiap kunci tepat satu baris.
Nilai berupa daftar/objek ditulis sebagai JSON dalam satu baris. Di dalam JSON, garis miring terbalik LaTeX ditulis ganda (\\frac, \\times).

:::callout
variant: tip
title: Judul singkat
text: Isi catatan.
:::
(variant salah satu dari: info, tip, warning, important. title boleh dihilangkan.)

:::table
caption: Keterangan tabel
headers: ["Kolom 1", "Kolom 2"]
rows: [["a", "b"], ["c", "d"]]
:::
(Tabel WAJIB memakai blok ini — tabel pipa "| a | b |" tidak didukung. Jumlah sel tiap baris sama dengan jumlah header. caption boleh dihilangkan.)

:::formula
latex: v = \frac{s}{t}
caption: Keterangan rumus
:::

:::steps
title: Langkah-langkah
items: ["Langkah pertama", "Langkah kedua"]
:::

:::definition
term: Istilah
definition: Pengertiannya.
:::

:::comparison
left_label: A
right_label: B
rows: [["ciri A", "ciri B"]]
:::

:::timeline
title: Judul
entries: [{"label": "1945", "text": "Peristiwa"}]
:::

:::toggle
title: Lihat pembahasan
content: Isi yang tersembunyi sampai diklik.
:::

:::columns
left: Isi kolom kiri
right: Isi kolom kanan
:::

:::flashcard
front: istilah
back: arti
:::

:::common_trap
term: kesalahan umum
explanation: kenapa itu keliru
:::

:::divider
:::

MEDIA — hanya dengan alamat https yang BENAR-BENAR diberikan penulis (di instruksi atau lampiran). Jangan pernah mengarang URL.
:::image  → kunci src, alt, caption
:::video  → kunci src, title
:::audio  → kunci src, title
:::embed  → kunci src, title
:::file   → kunci src, label
:::bookmark → kunci url, title

DILARANG: :::quiz, :::question_embed, soal latihan, pre-test. Soal dibuat terpisah di item Kuis."####;

fn language_rules(code: &str, name: &str) -> String {
    if code == "id" {
        format!(
            "## ATURAN BAHASA ##\nSemua isi WAJIB dalam {name}: judul modul, topik, judul bagian, tujuan bagian, dan isi bagian.\n\
PENGECUALIAN — biarkan dalam bahasa/notasi aslinya bila itu materi target yang sedang dipelajari: contoh kalimat dalam bahasa target (mis. kalimat Inggris di pelajaran Bahasa Inggris, kanji, kalimat Jerman), kosakata target, notasi baku (rumus, simbol kimia, kode, notasi musik), dan istilah teknis yang lazim dipakai dalam bahasa asing.\n\
Label pembungkus di sekitar materi target TETAP diterjemahkan: 'Contoh:', 'Latihan:', 'Aturan:', 'Catatan:', 'Definisi:', 'Rumus:', 'Kesimpulan:'."
        )
    } else {
        format!(
            "## OUTPUT LANGUAGE ##\nEvery field — module title, topic, section titles, goals and section bodies — MUST be written entirely in {name}.\n\
EXCEPTIONS — keep in original form when the element IS the target material being learned: target-language samples, target vocabulary, formal notation (formulas, chemistry, code) and established domain terms.\n\
Wrapper labels around examples (Example:, Exercise:, Rule:, Definition:, Note:, Formula:, Conclusion:) MUST be written in {name}."
        )
    }
}

/// The text between `open` and `close`; to the end when `close` is
/// missing, because a response cut off by the token limit still carries
/// a usable body.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let rest = &text[start..];
    Some(match rest.find(close) {
        Some(end) => &rest[..end],
        None => rest,
    })
}

fn parse_meta(raw: &str) -> serde_json::Value {
    serde_json::from_str(&strip_code_fence(raw.trim())).unwrap_or(serde_json::Value::Null)
}

fn meta_str(meta: &serde_json::Value, key: &str) -> String {
    meta.get(key).and_then(|v| v.as_str()).unwrap_or_default().trim().to_string()
}

fn meta_minutes(meta: &serde_json::Value) -> Option<i64> {
    let value = meta.get("minutes")?;
    value.as_i64().or_else(|| value.as_f64().map(|f| f.round() as i64)).or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

pub fn parse_generated_plan(text: &str) -> LessonPlan {
    let meta = between(text, PLAN_OPEN, PLAN_CLOSE).map(parse_meta).unwrap_or(serde_json::Value::Null);
    let mut sections = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(SECTION_OPEN) {
        let after = &rest[start + SECTION_OPEN.len()..];
        let (body, next) = match after.find(SECTION_CLOSE) {
            Some(end) => (&after[..end], &after[end + SECTION_CLOSE.len()..]),
            None => (after, ""),
        };
        let (meta_raw, content) = match body.find(CONTENT_OPEN) {
            Some(i) => (&body[..i], &body[i + CONTENT_OPEN.len()..]),
            None => (body, ""),
        };
        let section_meta = parse_meta(meta_raw);
        sections.push(LessonPlanSection {
            id: lesson_plan::new_section_id(),
            title: meta_str(&section_meta, "title"),
            minutes: meta_minutes(&section_meta),
            goal: meta_str(&section_meta, "goal"),
            content: lesson_plan::sanitize_generated(content),
        });
        rest = next;
    }
    LessonPlan {
        title: meta_str(&meta, "title"),
        topic: meta_str(&meta, "topic"),
        level: meta_str(&meta, "level"),
        language: String::new(),
        sections,
    }
}

/// META + CONTENT, the shape every single-section call returns. With
/// no delimiters at all the whole reply is taken as the body — the
/// same fallback parelabs uses, since a model that ignored the protocol
/// usually still wrote the section.
pub fn parse_section_reply(text: &str, fallback: &LessonPlanSection) -> Option<LessonPlanSection> {
    let meta = between(text, META_OPEN, META_CLOSE).map(parse_meta).unwrap_or(serde_json::Value::Null);
    let content = match between(text, CONTENT_OPEN, CONTENT_CLOSE) {
        Some(c) => c,
        None if !text.contains(META_OPEN) => text,
        None => return None,
    };
    let content = content.trim();
    if content.is_empty() && !fallback.content.trim().is_empty() {
        return None;
    }
    let title = meta_str(&meta, "title");
    let goal = meta_str(&meta, "goal");
    Some(LessonPlanSection {
        id: fallback.id.clone(),
        title: if title.is_empty() { fallback.title.clone() } else { title },
        minutes: meta_minutes(&meta).or(fallback.minutes),
        goal: if goal.is_empty() { fallback.goal.clone() } else { goal },
        content: lesson_plan::sanitize_generated(content),
    })
}

struct ItemContext {
    title: String,
    content_type: Option<String>,
    status: String,
    subject: Option<String>,
}

async fn item_context(pool: &PgPool, item_id: Uuid) -> Result<ItemContext, AppError> {
    sqlx::query_as!(
        ItemContext,
        r#"select i.title, i.content_type, i.status, s.name as "subject?"
           from module_items i
           join modules m on m.id = i.module_id
           left join subjects s on s.id = coalesce(i.subject_id, m.subject_id)
           where i.id = $1"#,
        item_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("module_item_not_found"))
}

fn ensure_editable_article(item: &ItemContext) -> Result<(), AppError> {
    if item.content_type.as_deref() != Some("article") {
        return Err(AppError::UnprocessableEntity("not_an_article", "hanya item Artikel yang punya Modul Belajar".to_string()));
    }
    if item.status == "published" {
        return Err(AppError::UnprocessableEntity(
            "cannot_edit_published_content",
            "a published module item cannot be edited in place — see ADR-0008".to_string(),
        ));
    }
    Ok(())
}

fn validation_error(code: &'static str, detail: &str) -> AppError {
    AppError::UnprocessableEntity(code, detail.to_string())
}

async fn record_failed(pool: &PgPool, id: Uuid, user_id: Uuid, task: &str, model: &str, prompt_id: &str) {
    if let Err(e) = ai_task::insert_failed(pool, id, user_id, task, PROVIDER, model, prompt_id).await {
        tracing::error!(error = ?e, %id, "failed to record failed ai_tasks row");
    }
}

// ── Generate a whole plan ───────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct GeneratePlanResponse {
    pub ai_task_id: Uuid,
    pub lesson_plan: LessonPlan,
}

pub fn generation_prompt(req: &GenerateLessonPlanRequest, subject: Option<&str>, language: &str) -> (String, String) {
    let subject_part = subject.map(|s| format!(" untuk mata pelajaran \"{s}\"")).unwrap_or_default();
    let system = format!(
        "{rules}\n\n\
Anda adalah perancang kurikulum yang menyusun \"Modul Belajar\"{subject_part} di platform belajar Indonesia. \
Satu modul terdiri dari beberapa bagian (section) berurutan. Isi yang SAMA dipakai dua cara:\n\
  • Mode baca mandiri — siswa membaca bagian demi bagian seperti artikel yang mendalam.\n\
  • Rujukan AI tutor — AI mengajarkan bagian demi bagian.\n\
Karena itu setiap bagian harus menjadi bahan ajar yang utuh: kaya penjelasan, contoh, dan ilustrasi.\n\n\
KEDALAMAN TIAP BAGIAN — DEFAULT: LENGKAP\n\
- 400-1500 kata per bagian; boleh lebih bila topiknya menuntut. Jangan dipersingkat secara artifisial.\n\
- 3-6 paragraf penjelasan yang jelas.\n\
- Banyak contoh konkret: 5-12 contoh kalimat, soal yang diselesaikan, atau potongan kode — sesuai mapelnya.\n\
- Minimal satu dari: tabel, daftar, perbandingan, langkah-langkah.\n\
- 1-3 blok :::callout untuk tips, peringatan, atau inti penting.\n\
- Bahasa: pasangan contoh dan terjemahan/glosnya. Matematika/sains: contoh soal diselesaikan langkah demi langkah beserta rumusnya. Pemrograman: contoh kode utuh yang bisa dijalankan. Agama: dalil/sumber dikutip utuh beserta rujukannya (mis. QS Al-Baqarah: 183, HR. Bukhari no. 1234).\n\
- Bahas kesalahan umum atau miskonsepsi bila relevan.\n\
- Alur tiap bagian: konsep → penjelasan → contoh → penerapan.\n\n\
{ALM_GUIDE}\n\n\
FORMAT KELUARAN — BERBATAS, WAJIB PERSIS\n\
Keluarkan HANYA blok-blok berikut. Tanpa kalimat pembuka atau penutup, tanpa pagar kode yang membungkus semuanya.\n\n\
{PLAN_OPEN}\n\
{{\"title\": \"judul modul\", \"topic\": \"topik yang dirumuskan ulang\", \"level\": \"tingkat\"}}\n\
{PLAN_CLOSE}\n\
{SECTION_OPEN}\n\
{{\"title\": \"judul bagian\", \"minutes\": 8, \"goal\": \"apa yang dikuasai siswa setelah bagian ini\"}}\n\
{CONTENT_OPEN}\n\
isi bagian dalam format ALM, ditulis apa adanya — BUKAN JSON, tanpa escape\n\
{SECTION_CLOSE}\n\
(ulangi blok {SECTION_OPEN} untuk setiap bagian)\n\n\
Hanya judul, menit, dan tujuan yang berupa JSON. Isi bagian ditulis mentah di antara {CONTENT_OPEN} dan {SECTION_CLOSE}.",
        rules = language_rules(&req.language, language),
    );

    let mut user = String::new();
    if let Some(notes) = req.notes.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        user.push_str(&format!(
            "═══════════════════════════════════════════\n\
INSTRUKSI KHUSUS DARI PENULIS — WAJIB DIPATUHI\n\
═══════════════════════════════════════════\n\
{notes}\n\n\
Bila bertentangan dengan parameter atau aturan di bawah, IKUTI BLOK INI.\n\n"
        ));
    }
    let level = req.level.as_deref().map(str::trim).filter(|l| !l.is_empty()).unwrap_or("tentukan sendiri dari topik");
    let count_line = match req.section_count {
        Some(n) => format!("Jumlah bagian: tepat {n} — berurutan 1..{n}, tidak boleh kurang atau lebih."),
        None => "Jumlah bagian: tentukan sendiri (biasanya 5-10). Bila penulis menyebut jumlah tertentu di instruksi khusus atau topik (mis. \"7 bagian\"), ikuti angka itu persis.".to_string(),
    };
    user.push_str(&format!(
        "Susun modul belajar yang komprehensif.\n\n\
Topik: {topic}\n\
Mata pelajaran: {subject}\n\
Durasi total: {duration} menit — jumlah menit semua bagian kira-kira sama dengan ini.\n\
Tingkat: {level}\n\
{count_line}\n\
Bahasa: {language}\n\n\
⛔ Modul HANYA berisi bacaan/pelajaran. JANGAN membuat bagian \"Latihan Evaluasi\", \"Tes Pemahaman\", atau \"Kuis\", dan jangan menulis soal pilihan ganda, benar-salah, atau isian. Soal dibuat terpisah di item Kuis.\n\n\
INGAT: semua judul, tujuan, dan isi ditulis dalam {language}; materi target (contoh, kosakata, notasi) tetap dalam bentuk aslinya.",
        topic = req.topic.trim(),
        subject = subject.unwrap_or("-"),
        duration = req.duration_minutes.clamp(5, 600),
    ));
    (system, user)
}

// POST /ai/generate-lesson-plan — returns the plan without saving it:
// the author reviews it in the editor, which already holds unsaved
// state, and saves like any other edit.
pub async fn generate_plan(pool: &PgPool, ai: &dyn AIProvider, model: &str, ctx: &AuthContext, req: GenerateLessonPlanRequest) -> Result<GeneratePlanResponse, AppError> {
    module_item::can_edit_item(pool, ctx, req.item_id).await?;
    let item = item_context(pool, req.item_id).await?;
    ensure_editable_article(&item)?;
    if req.topic.trim().is_empty() {
        return Err(validation_error("topic_required", "topik wajib diisi"));
    }
    let language = language_name(&req.language).ok_or_else(|| validation_error("invalid_language", "bahasa tidak dikenal"))?;
    let req = GenerateLessonPlanRequest { section_count: req.section_count.map(|n| n.clamp(2, 20)), ..req };

    let (system_prompt, user_prompt) = generation_prompt(&req, item.subject.as_deref(), language);
    let ai_task_id = Uuid::new_v4();
    let max_tokens = resolve_max_tokens(model, 32_000).await;
    let request = GenerationRequest { model: model.to_string(), system_prompt, user_prompt, temperature: 0.5, max_tokens, image_url: None, json_mode: false };

    // One retry — a transient provider failure or a reply with no
    // parseable sections both tend to be one-off flakiness, not a
    // reason to make the author wait a full minute for nothing and
    // click "Buat Modul" again themselves.
    let mut outcome = None;
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "lesson plan generation provider call failed");
                continue;
            }
        };
        let plan = parse_generated_plan(&generation.text);
        if plan.sections.is_empty() {
            tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "lesson plan generation returned no sections");
            continue;
        }
        outcome = Some((plan, generation));
        break;
    }
    let Some((mut plan, generation)) = outcome else {
        tracing::warn!(%ai_task_id, "lesson plan generation failed after retry");
        record_failed(pool, ai_task_id, ctx.user_id, "lesson_plan_generation", model, GENERATE_PROMPT_ID).await;
        return Err(AppError::AiOutputValidationFailed);
    };
    // The author's explicit pick wins over whatever the model echoed.
    plan.language = req.language.clone();
    if plan.title.is_empty() {
        plan.title = req.topic.trim().to_string();
    }
    if plan.topic.is_empty() {
        plan.topic = req.topic.trim().to_string();
    }
    if let Some(level) = req.level.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        plan.level = level.to_string();
    }
    let plan = lesson_plan::normalize(plan)?;

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "lesson_plan_generation", PROVIDER, model, GENERATE_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;
    Ok(GeneratePlanResponse { ai_task_id, lesson_plan: plan })
}

// ── Edit (or write) one section ─────────────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct EditSectionResponse {
    pub ai_task_id: Uuid,
    pub section: LessonPlanSection,
}

fn edit_system_prompt() -> String {
    format!(
        "Anda editor bagian modul belajar yang teliti. Penulis memberi instruksi bahasa sehari-hari; terapkan pada bagian yang diberikan, lalu kembalikan bagian itu utuh.\n\n\
ATURAN\n\
1. PERTAHANKAN mapel dan tingkatnya. Jangan mengganti topik atau sasaran pembaca kecuali diminta.\n\
2. TERAPKAN instruksi dengan setia. \"Tambah contoh\" berarti menambah contoh, bukan meringkas. \"Jadikan tabel\" berarti mengubahnya ke blok :::table tanpa membuang contohnya. \"Kurang sinkron dengan @1 @2\" berarti menyelaraskan istilah, kedalaman, dan gaya dengan bagian rujukan.\n\
3. KEMBALIKAN BAGIAN UTUH, bukan potongan perubahannya — isi Anda menggantikan isi lama seluruhnya.\n\
4. Pertahankan format yang sudah ada (blok khusus, rumus, kode), dan tambahkan blok bila instruksi memintanya.\n\
5. Bagian rujukan dan item rujukan adalah jangkar gaya — selaraskan, jangan disalin.\n\
6. Bila bagian masih kosong, TULIS isinya dari nol mengikuti instruksi, tujuan bagian, dan posisinya dalam modul.\n\n\
{ALM_GUIDE}\n\n\
FORMAT KELUARAN — BERBATAS, WAJIB PERSIS\n\
Keluarkan HANYA dua blok ini, tanpa teks lain:\n\n\
{META_OPEN}\n\
{{\"title\": \"...\", \"goal\": \"...\", \"minutes\": 8}}\n\
{META_CLOSE}\n\
{CONTENT_OPEN}\n\
isi bagian yang baru, utuh, dalam format ALM — mentah, tanpa escape\n\
{CONTENT_CLOSE}"
    )
}

fn excerpt(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(limit).collect::<String>())
    }
}

struct ReferencedItem {
    id: Uuid,
    title: String,
    content_type: Option<String>,
    lesson_plan: Option<serde_json::Value>,
    quiz_config: Option<serde_json::Value>,
}

/// `pub(crate)` so `quiz_generation.rs` can reuse the same "sibling
/// items in this module" resolution instead of a second copy — a quiz
/// group's `reference_module_item_ids` is the same idea as a lesson
/// section's, just phrased for a different consumer.
pub(crate) async fn referenced_items_block(pool: &PgPool, item_id: Uuid, ids: &[Uuid]) -> Result<String, AppError> {
    if ids.is_empty() {
        return Ok(String::new());
    }
    let ids: Vec<Uuid> = ids.iter().copied().take(5).collect();
    // Scoped to the same module on purpose: a reference is "stay
    // consistent with the item next to this one", and resolving any id
    // at all would let the composer read items the author cannot see.
    let rows = sqlx::query_as!(
        ReferencedItem,
        r#"select id, title, content_type, lesson_plan, quiz_config from module_items
           where id = any($1) and node_type = 'item'
             and module_id = (select module_id from module_items where id = $2)"#,
        &ids[..],
        item_id,
    )
    .fetch_all(pool)
    .await?;

    let mut out = String::from("ITEM RUJUKAN (item lain di modul yang sama — pakai untuk konsistensi lintas item, JANGAN salin isinya):\n");
    for row in rows {
        out.push_str(&format!("--- @@{} ({}) ---\n", row.title, row.content_type.as_deref().unwrap_or("item")));
        // A quiz item has no "reading" to hand over — what matters is
        // which questions already exist, so the model doesn't invent
        // near-duplicates of them in the item being generated.
        if row.content_type.as_deref() == Some("quiz") {
            let existing: Vec<String> = row
                .quiz_config
                .as_ref()
                .and_then(|v| quiz_config_schema::parse(v).ok())
                .map(|quiz| {
                    quiz.question_groups
                        .iter()
                        .flat_map(|g| g.questions.iter())
                        .filter_map(|q| q.prompt_text().map(|t| t.chars().take(80).collect::<String>()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !existing.is_empty() {
                out.push_str(&format!("Soal yang SUDAH ADA di kuis ini (jangan diulang):\n- {}\n", excerpt(&existing.join("\n- "), 3000)));
            }
            out.push('\n');
            continue;
        }
        let plan = row.lesson_plan.as_ref().and_then(|v| lesson_plan::parse(v).ok());
        let body = match plan {
            Some(plan) => {
                for (i, s) in plan.sections.iter().enumerate() {
                    out.push_str(&format!("  {}. {}{}\n", i + 1, s.title, if s.goal.is_empty() { String::new() } else { format!(" — {}", s.goal) }));
                }
                plan.sections.iter().map(|s| s.content.as_str()).collect::<Vec<_>>().join("\n\n")
            }
            None => module_item::find_content_blocks(pool, row.id)
                .await?
                .into_iter()
                .filter_map(|b| b.raw_source)
                .collect::<Vec<_>>()
                .join("\n\n"),
        };
        if !body.trim().is_empty() {
            out.push_str(&format!("Cuplikan isi:\n{}\n", excerpt(&body, 3000)));
        }
        out.push('\n');
    }
    Ok(out)
}

// POST /ai/edit-lesson-section — the editor sends its CURRENT (possibly
// unsaved) plan, so the instruction applies to what the author sees,
// not to the last saved copy.
pub async fn edit_section(pool: &PgPool, ai: &dyn AIProvider, model: &str, ctx: &AuthContext, req: EditLessonSectionRequest) -> Result<EditSectionResponse, AppError> {
    module_item::can_edit_item(pool, ctx, req.item_id).await?;
    let item = item_context(pool, req.item_id).await?;
    ensure_editable_article(&item)?;
    let instruction = req.instruction.trim();
    if instruction.is_empty() {
        return Err(validation_error("instruction_required", "instruksi wajib diisi"));
    }
    if instruction.chars().count() > 80_000 {
        return Err(validation_error("instruction_too_long", "instruksi dan lampiran terlalu panjang (maks. 80.000 karakter)"));
    }
    let plan = lesson_plan::parse(&req.lesson_plan)?;
    let section = plan.sections.get(req.section_index).cloned().ok_or_else(|| validation_error("section_not_found", "bagian tidak ditemukan"))?;
    let language = language_name(if plan.language.is_empty() { "id" } else { &plan.language }).unwrap_or("Bahasa Indonesia");

    let mut user = format!(
        "INSTRUKSI PENULIS:\n{instruction}\n\n\
BAHASA KELUARAN: {language}\n\n\
MODUL: {title}{topic}{level}{subject}\n\
Daftar bagian dalam modul ini:\n",
        title = if plan.title.is_empty() { item.title.as_str() } else { plan.title.as_str() },
        topic = if plan.topic.is_empty() { String::new() } else { format!(" — {}", plan.topic) },
        level = if plan.level.is_empty() { String::new() } else { format!(" (tingkat {})", plan.level) },
        subject = item.subject.as_deref().map(|s| format!(" · mapel {s}")).unwrap_or_default(),
    );
    for (i, s) in plan.sections.iter().enumerate() {
        let marker = if i == req.section_index { "  ← bagian yang diedit" } else { "" };
        user.push_str(&format!("  {}. {}{marker}\n", i + 1, if s.title.is_empty() { "(tanpa judul)" } else { &s.title }));
    }
    user.push_str(&format!(
        "\nBAGIAN SAAT INI (bagian {}):\nTITLE: {}\nGOAL: {}\nMINUTES: {}\nCONTENT:\n{}\n\n",
        req.section_index + 1,
        section.title,
        section.goal,
        section.minutes.unwrap_or(5),
        if section.content.is_empty() { "(masih kosong)" } else { &section.content },
    ));

    let mut referenced: Vec<usize> = req.referenced_sections.iter().copied().filter(|&i| i != req.section_index && i < plan.sections.len()).collect();
    referenced.sort_unstable();
    referenced.dedup();
    if !referenced.is_empty() {
        user.push_str("BAGIAN RUJUKAN (selaraskan gaya, kedalaman, dan istilahnya — JANGAN diduplikasi):\n");
        for i in referenced {
            let s = &plan.sections[i];
            user.push_str(&format!("--- @{} ({}) ---\nTujuan: {}\nCuplikan isi:\n{}\n\n", i + 1, s.title, s.goal, excerpt(&s.content, 2500)));
        }
    }
    user.push_str(&referenced_items_block(pool, req.item_id, &req.referenced_item_ids).await?);
    user.push_str(&format!("Kembalikan HANYA dua blok ({META_OPEN}…{META_CLOSE} dan {CONTENT_OPEN}…{CONTENT_CLOSE})."));

    let ai_task_id = Uuid::new_v4();
    let max_tokens = resolve_max_tokens(model, 16_000).await;
    let request = GenerationRequest { model: model.to_string(), system_prompt: edit_system_prompt(), user_prompt: user, temperature: 0.4, max_tokens, image_url: None, json_mode: false };

    // One retry, same reasoning as generate_plan above: a stray provider
    // hiccup or an unparseable/invalid reply is usually a one-off, and
    // retrying is cheaper than making the author re-click "Edit dengan AI".
    let mut outcome = None;
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "lesson section edit provider call failed");
                continue;
            }
        };
        let Some(updated) = parse_section_reply(&generation.text, &section) else {
            tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "lesson section edit output unusable");
            continue;
        };
        if lesson_plan::validate_content(&updated.content).is_err() {
            tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "lesson section edit produced invalid content");
            continue;
        }
        outcome = Some((updated, generation));
        break;
    }
    let Some((updated, generation)) = outcome else {
        tracing::warn!(%ai_task_id, "lesson section edit failed after retry");
        record_failed(pool, ai_task_id, ctx.user_id, "lesson_section_edit", model, EDIT_PROMPT_ID).await;
        return Err(AppError::AiOutputValidationFailed);
    };

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "lesson_section_edit", PROVIDER, model, EDIT_PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;
    Ok(EditSectionResponse { ai_task_id, section: updated })
}

// ── Translate for the learner ───────────────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct TranslatePlanResponse {
    pub lesson_plan: LessonPlan,
    /// False when the plan is already in the requested language.
    pub translated: bool,
    pub cached: bool,
}

fn translate_system_prompt(language: &str) -> String {
    format!(
        "Anda penerjemah bahan ajar. Terjemahkan SATU bagian modul belajar ke {language}.\n\n\
ATURAN\n\
1. Terjemahkan narasi, penjelasan, judul, tujuan, dan label pembungkus (Contoh:, Catatan:, Rumus:, Latihan:) ke {language}.\n\
2. JANGAN terjemahkan materi target yang sedang dipelajari: contoh kalimat bahasa asing di pelajaran bahasa, kosakata target, dalil/ayat beserta rujukannya, rumus/LaTeX, kode, nama variabel, dan URL.\n\
3. Pertahankan format ALM persis: nama blok (:::callout dan seterusnya), nama kunci (text:, title:, headers:, rows:), struktur JSON, jumlah sel tabel, pagar kode, dan tanda $...$. Yang diterjemahkan hanya NILAI teksnya.\n\
4. Jangan menambah, meringkas, atau membuang isi apa pun.\n\n\
FORMAT KELUARAN — HANYA dua blok ini, tanpa teks lain:\n\
{META_OPEN}\n\
{{\"title\": \"judul terjemahan\", \"goal\": \"tujuan terjemahan\"}}\n\
{META_CLOSE}\n\
{CONTENT_OPEN}\n\
isi terjemahan dalam format ALM, mentah\n\
{CONTENT_CLOSE}"
    )
}

async fn translate_section(ai: &dyn AIProvider, model: &str, language: &str, max_tokens: i64, section: &LessonPlanSection) -> Result<(LessonPlanSection, i64), ()> {
    let user = format!("TITLE: {}\nGOAL: {}\nCONTENT:\n{}", section.title, section.goal, section.content);
    let generation = ai
        .generate(GenerationRequest {
            model: model.to_string(),
            system_prompt: translate_system_prompt(language),
            user_prompt: user,
            temperature: 0.2,
            max_tokens,
            image_url: None,
            json_mode: false,
        })
        .await
        .map_err(|e| tracing::warn!(error = ?e, "section translation provider call failed"))?;
    let translated = parse_section_reply(&generation.text, section).ok_or(())?;
    // A translation that breaks the ALM would break the reader; the
    // original is always a safe fallback.
    lesson_plan::validate_content(&translated.content).map_err(|_| ())?;
    Ok((LessonPlanSection { minutes: section.minutes, ..translated }, generation.tokens_used.unwrap_or(0) as i64))
}

async fn translate_header(ai: &dyn AIProvider, model: &str, language: &str, plan: &LessonPlan) -> Option<(String, String)> {
    let generation = ai
        .generate(GenerationRequest {
            model: model.to_string(),
            system_prompt: format!(
                "Terjemahkan judul dan topik modul belajar ke {language}. Materi target (istilah bahasa asing yang sedang dipelajari, rumus) tetap dalam bentuk aslinya. Keluarkan HANYA JSON {{\"title\": \"...\", \"topic\": \"...\"}}."
            ),
            user_prompt: serde_json::json!({ "title": plan.title, "topic": plan.topic }).to_string(),
            temperature: 0.2,
            max_tokens: 800,
            image_url: None,
            json_mode: true,
        })
        .await
        .ok()?;
    let meta = parse_meta(&generation.text);
    Some((meta_str(&meta, "title"), meta_str(&meta, "topic")))
}

// POST /ai/translate-lesson-plan — open to anyone who can read the item
// (the learner is the caller). Always translates the SAVED plan, never a
// client-supplied one, and caches the result per language.
pub async fn translate_plan(pool: &PgPool, ai: &dyn AIProvider, model: &str, ctx: &AuthContext, req: TranslateLessonPlanRequest) -> Result<TranslatePlanResponse, AppError> {
    module_item::get_detail(pool, ctx, req.item_id).await?;
    let plan = lesson_plan::load(pool, req.item_id).await?.ok_or(AppError::NotFound("lesson_plan_not_found"))?;
    let target = req.target_language.trim().to_lowercase();
    let language = language_name(&target).ok_or_else(|| validation_error("invalid_language", "bahasa tidak dikenal"))?;
    let source = if plan.language.is_empty() { "id" } else { plan.language.as_str() };
    if source == target {
        return Ok(TranslatePlanResponse { lesson_plan: plan, translated: false, cached: false });
    }

    let hash = lesson_plan::source_hash(&plan);
    if let Some(cached) = lesson_plan::cached_translation(pool, req.item_id, &target, &hash).await? {
        return Ok(TranslatePlanResponse { lesson_plan: cached, translated: true, cached: true });
    }

    let ai_task_id = Uuid::new_v4();
    let max_tokens = resolve_max_tokens(model, 12_000).await;
    // Sections are independent, so they go out in parallel — a
    // 10-section module takes about as long as its longest section.
    let (header, sections) = tokio::join!(
        translate_header(ai, model, language, &plan),
        join_all(plan.sections.iter().map(|s| translate_section(ai, model, language, max_tokens, s)))
    );

    let mut tokens = 0i64;
    let mut failures = 0usize;
    let translated_sections: Vec<LessonPlanSection> = sections
        .into_iter()
        .zip(plan.sections.iter())
        .map(|(result, original)| match result {
            Ok((section, used)) => {
                tokens += used;
                section
            }
            Err(()) => {
                failures += 1;
                original.clone()
            }
        })
        .collect();

    if !plan.sections.is_empty() && failures == plan.sections.len() {
        record_failed(pool, ai_task_id, ctx.user_id, "lesson_plan_translation", model, TRANSLATE_PROMPT_ID).await;
        return Err(AppError::AiOutputValidationFailed);
    }

    let (title, topic) = header.unwrap_or_default();
    let translated = LessonPlan {
        title: if title.is_empty() { plan.title.clone() } else { title },
        topic: if topic.is_empty() { plan.topic.clone() } else { topic },
        level: plan.level.clone(),
        language: target.clone(),
        sections: translated_sections,
    };
    // A partly failed run is shown but not cached, so the next learner
    // gets a fresh attempt instead of inheriting the untranslated parts.
    if failures == 0 {
        lesson_plan::store_translation(pool, req.item_id, &target, &hash, &translated).await?;
    }
    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "lesson_plan_translation", PROVIDER, model, TRANSLATE_PROMPT_ID, Some(tokens as i32)).await?;
    Ok(TranslatePlanResponse { lesson_plan: translated, translated: true, cached: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_delimited_plan() {
        let text = "Berikut modulnya:\n<<<PLAN>>>\n{\"title\": \"Gerak Lurus\", \"topic\": \"GLB\", \"level\": \"SMA\"}\n<<<END_PLAN>>>\n\
<<<SECTION>>>\n{\"title\": \"Pengantar\", \"minutes\": 8, \"goal\": \"Paham GLB\"}\n<<<CONTENT>>>\nKecepatan $v = \\frac{s}{t}$ tetap.\n<<<END_SECTION>>>\n\
<<<SECTION>>>\n{\"title\": \"Contoh\", \"minutes\": \"12\", \"goal\": \"\"}\n<<<CONTENT>>>\n:::callout\nvariant: tip\ntext: Terpotong di";
        let plan = parse_generated_plan(text);
        assert_eq!(plan.title, "Gerak Lurus");
        assert_eq!(plan.sections.len(), 2);
        assert_eq!(plan.sections[0].minutes, Some(8));
        // The backslash survives because the body never went through JSON.
        assert!(plan.sections[0].content.contains("\\frac{s}{t}"));
        assert_eq!(plan.sections[1].minutes, Some(12));
        // A body cut off mid-directive is still saveable.
        assert!(plan.sections[1].content.contains("Terpotong di"));
        lesson_plan::validate_content(&plan.sections[1].content).unwrap();
    }

    #[test]
    fn section_reply_keeps_id_and_falls_back_on_missing_meta() {
        let original = LessonPlanSection { id: "abc".into(), title: "Lama".into(), minutes: Some(5), goal: "Tujuan".into(), content: "Isi lama.".into() };
        let reply = "<<<META>>>\n{\"title\": \"Baru\"}\n<<<END_META>>>\n<<<CONTENT>>>\nIsi **baru**.\n<<<END_CONTENT>>>";
        let out = parse_section_reply(reply, &original).unwrap();
        assert_eq!(out.id, "abc");
        assert_eq!(out.title, "Baru");
        assert_eq!(out.goal, "Tujuan");
        assert_eq!(out.minutes, Some(5));
        assert_eq!(out.content, "Isi **baru**.");
    }

    #[test]
    fn section_reply_without_delimiters_is_the_body() {
        let original = LessonPlanSection { id: "abc".into(), ..Default::default() };
        let out = parse_section_reply("Hanya isi.", &original).unwrap();
        assert_eq!(out.content, "Hanya isi.");
    }

    #[test]
    fn empty_reply_never_wipes_existing_content() {
        let original = LessonPlanSection { id: "abc".into(), content: "Isi penting.".into(), ..Default::default() };
        assert!(parse_section_reply("<<<META>>>{}<<<END_META>>><<<CONTENT>>>\n<<<END_CONTENT>>>", &original).is_none());
    }

    #[test]
    fn every_language_has_a_name() {
        for (code, _) in LANGUAGES {
            assert!(language_name(code).is_some());
        }
        assert!(language_name("xx").is_none());
    }
}
