// Phase 37 — generating a question group's contents with AI.
//
// The prompt is assembled in layers, most general first, so that adding
// a subtype or a presentation variant never means writing a new prompt:
//
//   1. SHAPE  — the JSON schema and its hard rules (quiz_shape.rs).
//   2. SUBTYPE — what kind of question to write, when it differs from
//      the shape's default.
//   3. VARIANT — extra demands created by how the group is presented or
//      scored (a drag-and-drop pool, a statement grid, weighted options).
//   4. CONTEXT — the passage/transcript already on the group, plus the
//      author's own `context_prompt`.
//
// The result is merged back into the group rather than replacing it: a
// generator that overwrote the author's passage or option pool every
// time would make "generate a few more questions" impossible.

use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::ai_provider::{resolve_max_tokens, strip_code_fence, AIProvider, GenerationRequest};
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::quiz_config::{value_to_key, QuizConfig, QuizQuestionGroup};
use crate::services::storage::AssetStorage;
use crate::services::{ai_task, quiz_config_schema, quiz_shape, quiz_subtype};

const PROVIDER: &str = "openrouter";
const PROMPT_ID: &str = "quiz_group_generation_v1";

/// What the run is for. Authoring is rarely "write the whole group
/// again" — far more often it is one more question, or fixing the one
/// question that came out wrong, or supplying the key for a stem the
/// author typed themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationMode {
    /// Throw away the group's questions and write `count` new ones.
    Replace,
    /// Keep what is there and add `count` more.
    Append,
    /// Rewrite one question, keeping its number so anything pointing at
    /// it (a table cell, a flow step) keeps pointing somewhere real.
    Rewrite,
    /// Fill in ONLY the answer key and explanation of one question,
    /// leaving its stem and choices exactly as the author wrote them.
    AnswerOnly,
}

impl GenerationMode {
    pub fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        Ok(match raw.unwrap_or("replace") {
            "replace" => Self::Replace,
            "append" => Self::Append,
            "rewrite" => Self::Rewrite,
            "answer_only" => Self::AnswerOnly,
            other => {
                return Err(AppError::UnprocessableEntity(
                    "invalid_mode",
                    format!(r#"mode "{other}" tidak dikenal (replace, append, rewrite, answer_only)"#),
                ))
            }
        })
    }

    fn targets_one_question(self) -> bool {
        matches!(self, Self::Rewrite | Self::AnswerOnly)
    }
}

pub struct QuizGenerationBlueprint {
    pub item_id: Uuid,
    pub group_id: String,
    pub mode: GenerationMode,
    /// Which question to act on. Required by `Rewrite` and `AnswerOnly`.
    pub question_number: Option<String>,
    /// How many questions to produce. Ignored when targeting one.
    pub count: i64,
    /// Overrides the group's stored `context_prompt` for this run.
    pub context_prompt: Option<String>,
    /// Overrides the group's stored `reference_module_item_ids` for this
    /// run. Empty means "use whatever the group already has saved".
    pub reference_module_item_ids: Vec<Uuid>,
    /// An image or scanned page to extract questions FROM, instead of
    /// inventing them. Must be an asset the caller can read.
    pub asset_id: Option<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct QuizGenerationResponse {
    pub ai_task_id: Uuid,
    pub status: &'static str,
    pub group_id: String,
    pub question_count: usize,
    /// Which shared resources the model returned and we merged in, so
    /// the author can see that a passage or option pool was written for
    /// them rather than wondering where it came from.
    pub emitted: Vec<String>,
}

fn find_group<'a>(config: &'a QuizConfig, group_id: &str) -> Option<&'a QuizQuestionGroup> {
    config.question_groups.iter().find(|g| g.group_id == group_id)
}

/// The passage a group actually reads from: its own, else its section's,
/// else the deck's.
fn resolved_passage(config: &QuizConfig, group: &QuizQuestionGroup) -> Option<String> {
    let own = group.passage.as_deref().filter(|s| !s.trim().is_empty());
    if let Some(p) = own {
        return Some(p.to_string());
    }
    let from_section = group
        .section_id
        .as_deref()
        .and_then(|id| config.find_section(id))
        .and_then(|s| s.passage.as_deref())
        .filter(|s| !s.trim().is_empty());
    from_section.or(config.passage.as_deref().filter(|s| !s.trim().is_empty())).map(str::to_string)
}

fn resolved_transcript(config: &QuizConfig, group: &QuizQuestionGroup) -> Option<String> {
    let own = group.audio.transcript.as_deref().filter(|s| !s.trim().is_empty());
    if let Some(t) = own {
        return Some(t.to_string());
    }
    group
        .section_id
        .as_deref()
        .and_then(|id| config.find_section(id))
        .and_then(|s| s.audio.transcript.as_deref())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// Whether this group is scored by option weights rather than a key.
fn is_weighted(group: &QuizQuestionGroup) -> bool {
    group.questions.iter().any(|q| q.option_scores.is_some())
        || group.context_prompt.as_deref().is_some_and(|c| c.contains("option_scores"))
}

pub fn build_prompt(
    config: &QuizConfig,
    group: &QuizQuestionGroup,
    info: &quiz_subtype::SubtypeInfo,
    mode: GenerationMode,
    target: Option<&serde_json::Value>,
    count: i64,
    context_prompt: Option<&str>,
    start_number: i64,
    from_image: bool,
    reference_block: &str,
) -> (String, String) {
    let spec = quiz_shape::spec(info.shape);

    let mut rules: Vec<String> = spec.rules.iter().map(|r| (*r).to_string()).collect();
    if let Some(hint) = quiz_shape::subtype_hint(info.id) {
        rules.push(hint.to_string());
    }
    for rule in quiz_shape::variant_rules(group.display_mode.as_deref(), is_weighted(group)) {
        rules.push(rule.to_string());
    }

    let language = config.language.as_deref().unwrap_or("id");
    let level = config.level.as_deref().unwrap_or("sesuai jenjang materi");

    let system = format!(
        "Anda adalah penyusun soal ujian untuk platform belajar Indonesia. Keluarkan HANYA JSON, tanpa prosa, tanpa pagar kode.\n\n\
Tipe soal: {label} — {description}\n\n\
Bentuk JSON yang WAJIB diikuti (ini contoh terisi, bukan placeholder):\n{schema}\n\n\
ATURAN:\n{rules}\n\n\
MUTU SOAL — ini yang membedakan soal ujian sungguhan dari soal asal jadi:\n\
- Tulis dalam bahasa: {language}. Tingkat: {level}.\n\
- Setiap soal WAJIB punya `explanation` yang menjelaskan MENGAPA kunci benar, dan — bila ada pilihan — mengapa pengecoh yang paling menggoda itu salah. Tulis sebagai pembahasan untuk siswa, bukan catatan untuk penulis soal.\n\
- Pengecoh harus mencerminkan kesalahan berpikir yang benar-benar sering terjadi (salah rumus, salah baca data, kesimpulan terlalu jauh), bukan pilihan yang jelas ngawur.\n\
- Jangan membuat soal yang bisa dijawab benar tanpa membaca stimulus.\n\
- Jangan mengulang soal yang sudah ada; variasikan tingkat kesulitan.\n\
- Untuk rumus, notasi matematika, atau simbol kimia, tulis dengan LaTeX di antara $...$.\n\
- PENTING: di dalam JSON, setiap garis miring terbalik LaTeX WAJIB ditulis ganda. Tulis \\\\times, \\\\frac, \\\\text — BUKAN \\times, karena JSON akan membacanya sebagai karakter kendali dan rumusnya rusak.\n\
- Nomor soal WAJIB berurutan mulai dari {start_number}.",
        label = info.label,
        description = spec.description,
        schema = spec.schema,
        rules = rules.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n"),
    );

    // Supplying a key for someone else's question is a different job
    // from writing questions, and needs its own instruction rather than
    // a footnote on the writing one.
    if mode == GenerationMode::AnswerOnly {
        let question_json = target.map(|t| serde_json::to_string_pretty(t).unwrap_or_default()).unwrap_or_default();
        let weighted = is_weighted(group);
        let system = format!(
            "Anda adalah penyusun kunci jawaban dan pembahasan untuk soal ujian. Keluarkan HANYA JSON, tanpa prosa, tanpa pagar kode.\n\nBentuk keluaran:\n{shape}\n\nATURAN:\n- JANGAN mengubah pertanyaan, pilihan, atau nomor soal. Anda HANYA menentukan {what}.\n- Pembahasan WAJIB menjelaskan MENGAPA kunci itu benar, dan — bila ada pilihan — mengapa pengecoh yang paling menggoda itu salah.\n- Tulis pembahasan sebagai penjelasan untuk siswa, bukan catatan untuk penulis soal.\n- Bila soal tidak bisa dijawab dari bacaan/konteks yang diberikan, katakan itu di `explanation` dan tetap berikan jawaban paling masuk akal.\n- Untuk rumus, gunakan LaTeX di antara $...$, dan di dalam JSON tulis garis miring terbalik ganda (\\\\frac, bukan \\frac).\n\nBahasa: {language}.",
            shape = if weighted {
                r#"{"option_scores": {"A": 5, "B": 3, "C": 2, "D": 1}, "explanation": "..."}"#
            } else {
                r#"{"answer": "B", "explanation": "..."}"#
            },
            what = if weighted { "`option_scores` (bobot 1-5 tiap pilihan)" } else { "`answer` dan `explanation`" },
        );

        let mut user = format!("SOAL yang harus diberi kunci dan pembahasan:\n{question_json}\n");
        if let Some(passage) = resolved_passage(config, group) {
            user.push_str(&format!("\nBACAAN rujukan:\n{passage}\n"));
        }
        if let Some(transcript) = resolved_transcript(config, group) {
            user.push_str(&format!("\nTRANSKRIP rujukan:\n{transcript}\n"));
        }
        if !group.options.is_empty() {
            let pool: Vec<&str> = group.options.iter().map(|o| o.text()).collect();
            user.push_str(&format!("\nJawaban harus salah satu dari pool ini: {}\n", pool.join(" | ")));
        }
        return (system, user);
    }

    let mut user = String::new();
    if from_image {
        user.push_str(
            "EKSTRAK soal dari gambar/dokumen terlampir — JANGAN mengarang soal baru. Salin pertanyaan, pilihan, dan (bila terlihat) kunci jawabannya apa adanya, lalu susun ke bentuk JSON di atas. Bila kunci jawaban tidak terlihat di sumber, tentukan sendiri jawaban yang benar dan katakan dasarnya di `explanation`.\n\n",
        );
    }
    match mode {
        GenerationMode::Rewrite => {
            let question_json = target.map(|t| serde_json::to_string_pretty(t).unwrap_or_default()).unwrap_or_default();
            user.push_str(&format!(
                "Tulis ULANG satu soal berikut agar lebih baik — pertahankan topik dan tingkat kesulitannya, perbaiki kejelasan, kualitas pengecoh, dan pembahasannya. Keluarkan tepat 1 soal.\n\nSOAL LAMA:\n{question_json}\n\n"
            ));
        }
        GenerationMode::Append => {
            user.push_str(&format!("Tambahkan {count} soal BARU ke grup ini.\n"));
        }
        _ => {
            user.push_str(&format!("Buat {count} soal.\n"));
        }
    }
    if let Some(instruction) = group.instruction.as_deref().filter(|s| !s.trim().is_empty()) {
        user.push_str(&format!("Instruksi yang dilihat siswa: {instruction}\n"));
    }
    if let Some(context) = context_prompt.filter(|s| !s.trim().is_empty()) {
        user.push_str(&format!("Panduan penulis: {context}\n"));
    }
    if !group.target_tests.is_empty() {
        user.push_str(&format!("Mengikuti format ujian: {}\n", group.target_tests.join(", ")));
    }
    if !reference_block.trim().is_empty() {
        user.push_str(&format!("\n{reference_block}\n"));
    }

    // Existing shared context is handed over verbatim so the questions
    // actually test it, instead of the model inventing a second passage.
    if let Some(passage) = resolved_passage(config, group) {
        user.push_str(&format!("\nBACAAN (soal harus menguji isi ini):\n{passage}\n"));
    } else if info.needs_passage && !from_image {
        user.push_str("\nBelum ada bacaan. Tulis juga bacaannya pada field `passage`, 300-500 kata, dan pastikan setiap soal bisa dijawab dari bacaan itu.\n");
    }
    if let Some(transcript) = resolved_transcript(config, group) {
        user.push_str(&format!("\nTRANSKRIP AUDIO (soal harus menguji isi ini):\n{transcript}\n"));
    }
    if !group.options.is_empty() {
        let pool: Vec<&str> = group.options.iter().map(|o| o.text()).collect();
        user.push_str(&format!("\nPOOL JAWABAN yang sudah ada (pakai ini, jangan ganti): {}\n", pool.join(" | ")));
    }
    if !group.questions.is_empty() {
        let existing: Vec<String> = group
            .questions
            .iter()
            .filter_map(|q| q.prompt_text().map(|t| t.chars().take(80).collect::<String>()))
            .collect();
        if !existing.is_empty() {
            user.push_str(&format!("\nSoal yang SUDAH ADA (jangan diulang):\n- {}\n", existing.join("\n- ")));
        }
    }

    (system, user)
}

/// Merge the model's output into the group. Questions are replaced;
/// shared resources are only filled in where the author left a gap.
fn merge_into_group(
    group: &mut serde_json::Value,
    generated: &serde_json::Value,
    emits: &[&str],
    start_number: i64,
    mode: GenerationMode,
    target_number: Option<&str>,
) -> (usize, Vec<String>) {
    let mut emitted = Vec::new();

    for key in emits {
        if mode == GenerationMode::AnswerOnly {
            break;
        }
        // `passage` may also arrive for a group whose passage lives on
        // its section; writing it onto the group is correct — that is
        // the override slot.
        let Some(value) = generated.get(*key) else { continue };
        let is_empty = match value {
            serde_json::Value::Null => true,
            serde_json::Value::String(s) => s.trim().is_empty(),
            serde_json::Value::Array(a) => a.is_empty(),
            _ => false,
        };
        if is_empty {
            continue;
        }
        let already_set = group.get(*key).is_some_and(|existing| match existing {
            serde_json::Value::Null => false,
            serde_json::Value::String(s) => !s.trim().is_empty(),
            serde_json::Value::Array(a) => !a.is_empty(),
            _ => true,
        });
        if already_set {
            continue;
        }
        group[*key] = value.clone();
        emitted.push((*key).to_string());
    }

    let mut questions = if mode == GenerationMode::AnswerOnly {
        Vec::new()
    } else {
        generated.get("questions").and_then(|q| q.as_array()).cloned().unwrap_or_default()
    };

    // The model is asked to number from `start_number`, but a wrong
    // number silently collides with another group and makes two
    // questions share one answer — so renumber rather than trust it.
    let mut next = start_number;
    for question in &mut questions {
        let Some(obj) = question.as_object_mut() else { continue };
        let ranged = obj
            .get("number")
            .map(|n| value_to_key(n))
            .and_then(|key| key.split_once('-').map(|(a, b)| (a.trim().parse::<i64>(), b.trim().parse::<i64>())))
            .and_then(|(a, b)| match (a, b) {
                (Ok(from), Ok(to)) if to >= from => Some(to - from + 1),
                _ => None,
            });
        match ranged {
            // A multi-mark question keeps its span, just rebased.
            Some(span) => {
                obj.insert("number".to_string(), serde_json::json!(format!("{}-{}", next, next + span - 1)));
                next += span;
            }
            None => {
                obj.insert("number".to_string(), serde_json::json!(next));
                next += 1;
            }
        }
    }

    let existing = group.get("questions").and_then(|q| q.as_array()).cloned().unwrap_or_default();

    let merged = match mode {
        GenerationMode::Replace => questions,
        GenerationMode::Append => {
            let mut all = existing;
            all.extend(questions);
            all
        }
        GenerationMode::Rewrite => {
            // The new question takes the OLD number: a table cell or flow
            // step pointing at it must keep pointing somewhere real.
            let Some(mut replacement) = questions.into_iter().next() else { return (0, emitted) };
            if let (Some(obj), Some(number)) = (replacement.as_object_mut(), target_number) {
                obj.insert("number".to_string(), number_value(number));
            }
            existing
                .into_iter()
                .map(|q| {
                    if target_number.is_some_and(|n| value_to_key(q.get("number").unwrap_or(&serde_json::Value::Null)) == n) {
                        replacement.clone()
                    } else {
                        q
                    }
                })
                .collect()
        }
        GenerationMode::AnswerOnly => {
            // Only the key and the explanation are taken. The stem and
            // choices belong to the author and are never touched — that
            // is the entire promise of this mode.
            existing
                .into_iter()
                .map(|mut q| {
                    let is_target = target_number.is_some_and(|n| value_to_key(q.get("number").unwrap_or(&serde_json::Value::Null)) == n);
                    if !is_target {
                        return q;
                    }
                    if let Some(obj) = q.as_object_mut() {
                        for field in ["answer", "explanation", "option_scores"] {
                            if let Some(value) = generated.get(field) {
                                obj.insert(field.to_string(), value.clone());
                            }
                        }
                    }
                    q
                })
                .collect()
        }
    };

    let count = if mode.targets_one_question() { 1 } else { merged.len() };
    group["questions"] = serde_json::Value::Array(merged);
    (count, emitted)
}

/// A question number is an integer when it can be, a string when it is a
/// multi-mark range like "5-6".
fn number_value(key: &str) -> serde_json::Value {
    key.parse::<i64>().map(serde_json::Value::from).unwrap_or_else(|_| serde_json::Value::String(key.to_string()))
}

/// The lowest number this group may use without colliding with any other
/// group in the deck.
fn start_number_for(config: &QuizConfig, group_id: &str) -> i64 {
    let highest = config
        .question_groups
        .iter()
        .filter(|g| g.group_id != group_id)
        .flat_map(|g| g.questions.iter())
        .filter_map(|q| {
            let key = value_to_key(&q.number);
            key.split('-').next_back().and_then(|s| s.trim().parse::<i64>().ok())
        })
        .max();
    highest.map_or(1, |n| n + 1)
}

/// Undo the damage a single-escaped LaTeX command does inside JSON.
///
/// A model that writes `"$1000 \times 3$"` in JSON has written a TAB:
/// `\t` is a JSON escape, so `\times` decodes to U+0009 followed by
/// "imes" and the formula renders as garbage. The same happens to
/// `\frac` (formfeed + "rac") and `\beta` (backspace + "eta").
///
/// A control character followed immediately by a letter has no
/// legitimate place in exam prose, so restoring the backslash is safe.
/// Newlines are deliberately left alone — an explanation may genuinely
/// span lines — and a CR is only repaired when it is not part of CRLF.
fn repair_latex_escapes(value: serde_json::Value) -> serde_json::Value {
    fn fix(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let restored = match c {
                '\t' => Some('t'),
                '\u{8}' => Some('b'),
                '\u{c}' => Some('f'),
                '\r' if chars.get(i + 1) != Some(&'\n') => Some('r'),
                _ => None,
            };
            match restored {
                Some(letter) if chars.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic()) => {
                    out.push('\\');
                    out.push(letter);
                }
                _ => out.push(c),
            }
            i += 1;
        }
        out
    }

    match value {
        serde_json::Value::String(s) => serde_json::Value::String(fix(&s)),
        serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(repair_latex_escapes).collect()),
        serde_json::Value::Object(map) => {
            serde_json::Value::Object(map.into_iter().map(|(k, v)| (k, repair_latex_escapes(v))).collect())
        }
        other => other,
    }
}

async fn record_failed(pool: &PgPool, ai_task_id: Uuid, user_id: Uuid, model: &str) {
    if let Err(e) = ai_task::insert_failed(pool, ai_task_id, user_id, "quiz_group_generation", PROVIDER, model, PROMPT_ID).await {
        tracing::error!(error = ?e, %ai_task_id, "failed to record failed ai_tasks row");
    }
}

// POST /ai/generate-quiz-group
pub async fn generate_quiz_group(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    ctx: &AuthContext,
    model: &str,
    bp: QuizGenerationBlueprint,
) -> Result<QuizGenerationResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    if !bp.mode.targets_one_question() && (bp.count < 1 || bp.count > 50) {
        return Err(AppError::UnprocessableEntity("invalid_count", "count harus antara 1 dan 50".to_string()));
    }
    if bp.mode.targets_one_question() && bp.question_number.is_none() {
        return Err(AppError::UnprocessableEntity(
            "question_number_required",
            "mode ini butuh question_number soal yang dituju".to_string(),
        ));
    }

    let raw_config = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, bp.item_id)
        .fetch_optional(pool)
        .await?
        .flatten()
        .ok_or(AppError::NotFound("quiz_config_not_found"))?;
    let quiz: QuizConfig = quiz_config_schema::parse(&raw_config)?;
    let group = find_group(&quiz, &bp.group_id).ok_or(AppError::NotFound("question_group_not_found"))?;
    let info = quiz_subtype::find(&group.r#type).ok_or_else(|| {
        AppError::UnprocessableEntity("unknown_subtype", format!("subtype \"{}\" tidak dikenal", group.r#type))
    })?;

    // A group whose questions a human writes by hand (an H5P embed, a
    // live speaking activity) has nothing for the model to produce.
    if matches!(info.shape, quiz_subtype::SubtypeShape::InteractiveEmbed) {
        return Err(AppError::UnprocessableEntity(
            "not_generatable",
            format!("tipe \"{}\" disiapkan manual, bukan lewat AI", info.label),
        ));
    }

    // A source image turns this from "invent questions" into "extract
    // the questions on this page", which is a different instruction.
    let image_url = match bp.asset_id {
        Some(asset_id) => {
            let access = crate::services::drive_permissions::resolve_access(pool, ctx, crate::services::drive_permissions::DriveResource::Asset, asset_id).await?;
            if access.is_none() {
                return Err(AppError::NotFound("asset_not_found"));
            }
            let asset = crate::services::asset::find_by_id(pool, asset_id).await?.ok_or(AppError::NotFound("asset_not_found"))?;
            if !asset.r#type.starts_with("image/") {
                return Err(AppError::UnprocessableEntity(
                    "unsupported_source",
                    "sumber harus berupa gambar (foto atau hasil pindai halaman soal)".to_string(),
                ));
            }
            // A signed url the model's fetcher can actually reach.
            Some(
                storage
                    .signed_url(&asset.id.to_string(), config.asset_signed_url_ttl_seconds as u64)
                    .await
                    .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?,
            )
        }
        None => None,
    };

    // Appending must not reuse the numbers already in THIS group, while
    // replacing frees them — so the floor differs by mode.
    let start_number = match bp.mode {
        GenerationMode::Append => start_number_for(&quiz, "").max(start_number_for(&quiz, &bp.group_id)),
        _ => start_number_for(&quiz, &bp.group_id),
    };

    // The question being rewritten or answered, handed to the model
    // verbatim so it works from what the author actually wrote.
    let target_question = match (&bp.question_number, bp.mode.targets_one_question()) {
        (Some(number), true) => {
            let found = group.questions.iter().find(|q| value_to_key(&q.number) == *number);
            if found.is_none() {
                return Err(AppError::NotFound("question_not_found"));
            }
            found.map(|q| serde_json::to_value(q).unwrap_or(serde_json::Value::Null))
        }
        _ => None,
    };

    let context_prompt = bp.context_prompt.as_deref().or(group.context_prompt.as_deref());

    let reference_ids: Vec<Uuid> = if !bp.reference_module_item_ids.is_empty() {
        bp.reference_module_item_ids.clone()
    } else {
        group.reference_module_item_ids.iter().filter_map(|id| Uuid::parse_str(id).ok()).collect()
    };
    let reference_block = crate::services::lesson_plan_ai::referenced_items_block(pool, bp.item_id, &reference_ids).await?;

    let (system_prompt, user_prompt) = build_prompt(
        &quiz,
        group,
        info,
        bp.mode,
        target_question.as_ref(),
        bp.count,
        context_prompt,
        start_number,
        image_url.is_some(),
        &reference_block,
    );

    let ai_task_id = Uuid::new_v4();
    let requested = if bp.mode.targets_one_question() { 1 } else { bp.count };
    let max_tokens = resolve_max_tokens(model, (500 * requested).max(1500)).await;
    let request = GenerationRequest {
        model: model.to_string(),
        system_prompt,
        user_prompt,
        temperature: 0.5,
        max_tokens,
        image_url,
        // The whole contract here is a JSON object, so ask for one.
        json_mode: true,
    };

    // One retry, on either a transient provider failure or output that
    // didn't come back as valid JSON — both are exactly the kind of
    // one-off flakiness a second attempt tends to clear, and neither
    // needed the merge below to happen first to detect.
    let mut outcome = None;
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "quiz group generation provider call failed");
                continue;
            }
        };
        match serde_json::from_str::<serde_json::Value>(&strip_code_fence(&generation.text)) {
            Ok(value) => {
                let parsed = repair_latex_escapes(value);
                outcome = Some((parsed, generation));
                break;
            }
            Err(_) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "quiz generation output was not valid JSON");
            }
        }
    }
    let Some((parsed, generation)) = outcome else {
        tracing::warn!(%ai_task_id, "quiz group generation failed after retry");
        record_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed);
    };

    // Merge into the RAW json so fields this Rust struct does not model
    // (a future layout's) survive untouched.
    let mut next_config = raw_config.clone();
    let Some(groups) = next_config.get_mut("question_groups").and_then(|g| g.as_array_mut()) else {
        return Err(AppError::Internal(anyhow::anyhow!("quiz_config lost its question_groups")));
    };
    let Some(target) = groups.iter_mut().find(|g| g.get("group_id").and_then(|v| v.as_str()) == Some(bp.group_id.as_str())) else {
        return Err(AppError::NotFound("question_group_not_found"));
    };
    let spec = quiz_shape::spec(info.shape);
    let (question_count, emitted) = merge_into_group(
        target,
        &parsed,
        spec.emits,
        start_number,
        bp.mode,
        bp.question_number.as_deref(),
    );

    if question_count == 0 {
        tracing::warn!(%ai_task_id, raw_output = %generation.text, "quiz generation returned no questions");
        record_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed);
    }

    // Refuse to save something the engine could not then dispatch — a
    // duplicate number or a dangling reference would corrupt every
    // attempt on this paper.
    quiz_config_schema::validate_structure(&next_config).map_err(|e| {
        tracing::warn!(error = ?e, %ai_task_id, "quiz generation produced an invalid config");
        e
    })?;

    sqlx::query!(r#"update module_items set quiz_config = $2 where id = $1"#, bp.item_id, next_config)
        .execute(pool)
        .await?;

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "quiz_group_generation", PROVIDER, model, PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;

    Ok(QuizGenerationResponse { ai_task_id, status: "done", group_id: bp.group_id, question_count, emitted })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config_with(group: serde_json::Value) -> QuizConfig {
        serde_json::from_value(json!({ "sections": [], "question_groups": [group] })).unwrap()
    }

    #[test]
    fn append_keeps_what_is_already_there() {
        let mut group = json!({"group_id": "g", "type": "multiple_choice", "questions": [{"number": 1, "stem": "lama"}]});
        let generated = json!({"questions": [{"number": 99, "stem": "baru"}]});
        let (count, _) = merge_into_group(&mut group, &generated, &[], 2, GenerationMode::Append, None);
        assert_eq!(count, 2);
        let stems: Vec<&str> = group["questions"].as_array().unwrap().iter().map(|q| q["stem"].as_str().unwrap()).collect();
        assert_eq!(stems, vec!["lama", "baru"]);
        // The appended one is renumbered onto the next free slot.
        assert_eq!(group["questions"][1]["number"], json!(2));
    }

    #[test]
    fn rewrite_replaces_one_question_and_keeps_its_number() {
        // The number is load-bearing: a table cell or flow step points at
        // it, so a rewrite that renumbered would strand that reference.
        let mut group = json!({"group_id": "g", "type": "multiple_choice", "questions": [
            {"number": 5, "stem": "lama A"}, {"number": 6, "stem": "lama B"},
        ]});
        let generated = json!({"questions": [{"number": 1, "stem": "tulisan ulang"}]});
        let (count, _) = merge_into_group(&mut group, &generated, &[], 7, GenerationMode::Rewrite, Some("5"));
        assert_eq!(count, 1);
        let questions = group["questions"].as_array().unwrap();
        assert_eq!(questions.len(), 2, "rewriting must not add or drop questions");
        assert_eq!(questions[0]["number"], json!(5));
        assert_eq!(questions[0]["stem"], json!("tulisan ulang"));
        assert_eq!(questions[1]["stem"], json!("lama B"), "the other question is untouched");
    }

    #[test]
    fn answer_only_never_touches_the_authors_question() {
        // The whole promise of this mode: the author's wording and
        // options survive exactly, only the key and explanation land.
        let mut group = json!({"group_id": "g", "type": "multiple_choice", "questions": [{
            "number": 3,
            "stem": "Pertanyaan tulisan penulis",
            "choices": [{"label": "A", "text": "satu"}, {"label": "B", "text": "dua"}],
        }]});
        let generated = json!({
            "answer": "B",
            "explanation": "Karena dua.",
            "stem": "PERTANYAAN KARANGAN MODEL",
            "choices": [{"label": "A", "text": "diganti"}],
        });
        let (count, emitted) = merge_into_group(&mut group, &generated, &["passage"], 1, GenerationMode::AnswerOnly, Some("3"));
        assert_eq!(count, 1);
        let q = &group["questions"][0];
        assert_eq!(q["stem"], json!("Pertanyaan tulisan penulis"));
        assert_eq!(q["choices"][0]["text"], json!("satu"));
        assert_eq!(q["choices"].as_array().unwrap().len(), 2);
        assert_eq!(q["answer"], json!("B"));
        assert_eq!(q["explanation"], json!("Karena dua."));
        assert!(emitted.is_empty(), "answer_only must not merge shared resources either");
    }

    #[test]
    fn answer_only_leaves_other_questions_alone() {
        let mut group = json!({"group_id": "g", "type": "multiple_choice", "questions": [
            {"number": 1, "stem": "a"}, {"number": 2, "stem": "b", "answer": "X"},
        ]});
        let generated = json!({"answer": "C", "explanation": "..."});
        merge_into_group(&mut group, &generated, &[], 1, GenerationMode::AnswerOnly, Some("1"));
        assert_eq!(group["questions"][0]["answer"], json!("C"));
        assert_eq!(group["questions"][1]["answer"], json!("X"), "question 2 keeps its own key");
    }

    #[test]
    fn answer_only_carries_weights_for_a_weighted_question() {
        let mut group = json!({"group_id": "g", "type": "multiple_choice", "questions": [{"number": 1, "stem": "situasi"}]});
        let generated = json!({"option_scores": {"A": 5, "B": 2}, "explanation": "A paling tepat."});
        merge_into_group(&mut group, &generated, &[], 1, GenerationMode::AnswerOnly, Some("1"));
        assert_eq!(group["questions"][0]["option_scores"]["A"], json!(5));
    }

    #[test]
    fn the_answer_only_prompt_forbids_changing_the_question() {
        let config = config_with(json!({
            "group_id": "g", "type": "multiple_choice",
            "questions": [{"number": 1, "stem": "Berapa 2+2?", "choices": [{"label": "A", "text": "4"}]}],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let target = serde_json::to_value(&group.questions[0]).unwrap();
        let (system, user) = build_prompt(&config, group, info, GenerationMode::AnswerOnly, Some(&target), 1, None, 1, false, "");
        assert!(system.contains("JANGAN mengubah pertanyaan"));
        assert!(user.contains("Berapa 2+2?"), "the model must see the question it is answering");
    }

    #[test]
    fn the_rewrite_prompt_shows_the_old_question() {
        let config = config_with(json!({
            "group_id": "g", "type": "multiple_choice",
            "questions": [{"number": 1, "stem": "Soal yang kurang jelas"}],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let target = serde_json::to_value(&group.questions[0]).unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Rewrite, Some(&target), 1, None, 1, false, "");
        assert!(user.contains("Tulis ULANG"));
        assert!(user.contains("Soal yang kurang jelas"));
    }

    #[test]
    fn an_unknown_mode_is_refused() {
        assert!(GenerationMode::parse(Some("bikin_semua")).is_err());
        assert_eq!(GenerationMode::parse(None).unwrap(), GenerationMode::Replace);
    }

    #[test]
    fn mangled_latex_commands_are_restored() {
        // What a model actually produced: `\times` written with a single
        // backslash inside JSON, which decodes to TAB + "imes".
        // Built explicitly from the control character so the test does
        // not depend on how this source file escapes.
        let repaired = repair_latex_escapes(json!({"explanation": format!("F = 1000 {}imes 3", '\t')}));
        assert_eq!(repaired["explanation"], json!("F = 1000 \\times 3"));

        let frac = repair_latex_escapes(json!({"stem": format!("{}rac{{a}}{{b}}", '\u{c}')}));
        assert_eq!(frac["stem"], json!("\\frac{a}{b}"));

        let beta = repair_latex_escapes(json!({"stem": format!("{}eta", '\u{8}')}));
        assert_eq!(beta["stem"], json!("\\beta"));
    }

    #[test]
    fn repair_leaves_legitimate_whitespace_alone() {
        // A genuine multi-line explanation must survive untouched, and a
        // tab that is not part of a command is not a broken formula.
        let value = repair_latex_escapes(json!({"explanation": "Baris satu\nBaris dua", "x": format!("kolom{}\t2", '\t')}));
        assert_eq!(value["explanation"], json!("Baris satu\nBaris dua"));
        assert!(value["x"].as_str().unwrap().contains('\t'), "a tab before a non-letter stays a tab");
    }

    #[test]
    fn repair_reaches_nested_questions() {
        let value = repair_latex_escapes(json!({
            "questions": [{"number": 1, "explanation": format!("{}heta", '\t')}],
        }));
        assert_eq!(value["questions"][0]["explanation"], json!("\\theta"));
    }

    #[test]
    fn the_prompt_warns_about_json_escaping() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (system, _) = build_prompt(&config, group, info, GenerationMode::Replace, None, 1, None, 1, false, "");
        assert!(system.contains("ditulis ganda"), "the prompt must demand double-escaped backslashes");
    }

    #[test]
    fn numbering_starts_after_every_other_group() {
        let config: QuizConfig = serde_json::from_value(json!({
            "sections": [],
            "question_groups": [
                {"group_id": "a", "type": "multiple_choice", "questions": [{"number": 1}, {"number": 2}]},
                {"group_id": "b", "type": "multiple_choice", "questions": [{"number": "7-8"}]},
                {"group_id": "target", "type": "multiple_choice", "questions": []},
            ],
        }))
        .unwrap();
        // 8 is the highest number in use anywhere else, so the group
        // being regenerated starts at 9 — its own old numbers are freed.
        assert_eq!(start_number_for(&config, "target"), 9);
    }

    #[test]
    fn generated_questions_are_renumbered_not_trusted() {
        // The model was told to start at 5 but answered 1,2 — left alone
        // that silently collides with another group.
        let mut group = json!({"group_id": "g", "type": "multiple_choice", "questions": []});
        let generated = json!({"questions": [{"number": 1, "stem": "a"}, {"number": 2, "stem": "b"}]});
        let (count, _) = merge_into_group(&mut group, &generated, &[], 5, GenerationMode::Replace, None);
        assert_eq!(count, 2);
        let numbers: Vec<i64> = group["questions"].as_array().unwrap().iter().map(|q| q["number"].as_i64().unwrap()).collect();
        assert_eq!(numbers, vec![5, 6]);
    }

    #[test]
    fn a_multi_mark_range_keeps_its_span_when_renumbered() {
        let mut group = json!({"group_id": "g", "type": "multiple_choice_multiple", "questions": []});
        let generated = json!({"questions": [{"number": "1-2", "stem": "pilih dua"}, {"number": 3, "stem": "biasa"}]});
        let (_, _) = merge_into_group(&mut group, &generated, &[], 10, GenerationMode::Replace, None);
        let numbers: Vec<String> = group["questions"].as_array().unwrap().iter().map(|q| value_to_key(&q["number"])).collect();
        assert_eq!(numbers, vec!["10-11", "12"]);
    }

    #[test]
    fn an_authored_passage_is_never_overwritten() {
        // Regenerating questions must not silently replace the passage
        // the author already wrote and the other groups depend on.
        let mut group = json!({"group_id": "g", "type": "true_false_not_given", "passage": "Bacaan asli penulis.", "questions": []});
        let generated = json!({"passage": "Bacaan karangan model.", "questions": [{"number": 1, "text": "x", "answer": "True"}]});
        let (_, emitted) = merge_into_group(&mut group, &generated, &["passage"], 1, GenerationMode::Replace, None);
        assert_eq!(group["passage"], json!("Bacaan asli penulis."));
        assert!(emitted.is_empty());
    }

    #[test]
    fn a_missing_passage_is_filled_in_and_reported() {
        let mut group = json!({"group_id": "g", "type": "true_false_not_given", "questions": []});
        let generated = json!({"passage": "Bacaan baru.", "questions": [{"number": 1, "text": "x", "answer": "True"}]});
        let (_, emitted) = merge_into_group(&mut group, &generated, &["passage"], 1, GenerationMode::Replace, None);
        assert_eq!(group["passage"], json!("Bacaan baru."));
        assert_eq!(emitted, vec!["passage".to_string()]);
    }

    #[test]
    fn the_prompt_carries_the_shape_rules_and_demands_explanations() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (system, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, Some("Tentang fotosintesis"), 1, false, "");
        assert!(system.contains("explanation"), "prompt must demand explanations");
        assert!(system.contains("mengapa pengecoh"), "prompt must demand distractor analysis");
        assert!(system.contains("Tepat 4 pilihan"), "shape rules must be included");
        assert!(user.contains("Buat 3 soal"));
        assert!(user.contains("Tentang fotosintesis"));
    }

    #[test]
    fn a_weighted_group_is_told_not_to_write_an_answer_key() {
        let config = config_with(json!({
            "group_id": "g", "type": "multiple_choice",
            "questions": [{"number": 1, "option_scores": {"A": 5}}],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (system, _) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, "");
        assert!(system.contains("option_scores"));
        assert!(system.contains("JANGAN tulis `answer`"));
    }

    #[test]
    fn a_drag_and_drop_group_is_told_to_emit_an_option_pool() {
        let config = config_with(json!({"group_id": "g", "type": "matching", "display_mode": "ielts", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("matching").unwrap();
        let (system, _) = build_prompt(&config, group, info, GenerationMode::Replace, None, 4, None, 1, false, "");
        assert!(system.contains("word bank"), "the drag-and-drop variant rule must be present");
    }

    #[test]
    fn an_existing_passage_is_handed_to_the_model_instead_of_asking_for_a_new_one() {
        let config = config_with(json!({
            "group_id": "g", "type": "true_false_not_given",
            "passage": "Lebah menyerbuki 70% tanaman pangan.",
            "questions": [],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("true_false_not_given").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, "");
        assert!(user.contains("Lebah menyerbuki"));
        assert!(!user.contains("Tulis juga bacaannya"));
    }

    #[test]
    fn image_mode_switches_from_inventing_to_extracting() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 5, None, 1, true, "");
        assert!(user.contains("EKSTRAK"), "image mode must tell the model to extract, not invent");
        assert!(user.contains("JANGAN mengarang"));
    }

    #[test]
    fn a_reference_block_is_folded_into_the_user_prompt() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(
            &config,
            group,
            info,
            GenerationMode::Replace,
            None,
            3,
            None,
            1,
            false,
            "ITEM RUJUKAN (item lain di modul yang sama — pakai untuk konsistensi lintas item, JANGAN salin isinya):\n--- @@Kuis Sebelumnya (quiz) ---\nSoal yang SUDAH ADA di kuis ini (jangan diulang):\n- Apa ibu kota Indonesia?\n",
        );
        assert!(user.contains("ITEM RUJUKAN"));
        assert!(user.contains("Apa ibu kota Indonesia?"));
    }

    #[test]
    fn an_empty_reference_block_adds_nothing() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, "");
        assert!(!user.contains("ITEM RUJUKAN"));
    }

    #[test]
    fn existing_questions_are_listed_so_they_are_not_repeated() {
        let config = config_with(json!({
            "group_id": "g", "type": "multiple_choice",
            "questions": [{"number": 1, "stem": "Apa ibu kota Indonesia?"}],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 2, None, 2, false, "");
        assert!(user.contains("jangan diulang"));
        assert!(user.contains("Apa ibu kota Indonesia?"));
    }
}
