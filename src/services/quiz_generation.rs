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

use futures_util::stream::{self, StreamExt};
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

/// How many groups' generation calls (each a real network round trip to
/// the model) run at once. Capped rather than unbounded so a 20-group
/// paper doesn't open 20 simultaneous provider connections — and DB
/// writes stay safe either way, each one taking its own row lock.
const BATCH_CONCURRENCY: usize = 3;

const PROVIDER: &str = "openrouter";

/// Gemini's thinking tokens come out of the same budget as the answer.
/// A quiz batch is a structured, well-specified job — it needs a little
/// planning room, not an open-ended budget that can eat the whole reply
/// and return `MAX_TOKENS` with nothing in it.
const QUIZ_THINKING_BUDGET: i64 = 512;
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
    /// "Tempel & Parse" — pasted text to extract questions FROM (a PDF's
    /// copied text, an old worksheet), instead of inventing new ones.
    /// Same idea as `asset_id` but for text the author already has
    /// rather than a photographed page. Ignored when `asset_id` is set.
    pub raw_text: Option<String>,
    /// "Ubah tipe soal" — reshape this group into a DIFFERENT subtype
    /// instead of regenerating in its current one. When set, the prompt
    /// is built from the NEW subtype's shape (not the group's stored
    /// one), and the merge writes the new subtype id onto the group
    /// alongside its regenerated questions.
    pub convert_to_subtype: Option<String>,
    /// Fase 5d — stamp `ai_meta.draft = true` on the written group. See
    /// `GenerateQuizGroupRequest::mark_draft`.
    pub mark_draft: bool,
    /// Exactly what each question must be (bank filling). Empty = let the
    /// taxonomy's own per-call quota decide.
    pub slots: Vec<crate::models::requests::ai::QuestionSlotRequest>,
    /// Restrict a referenced Modul Belajar to one of its sections.
    pub reference_section_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct QuizGenerationResponse {
    pub ai_task_id: Uuid,
    pub status: &'static str,
    pub group_id: String,
    pub question_count: usize,
    /// How many generated questions were thrown away for repeating a
    /// question the item already had. A bank filler asks for this many
    /// more rather than leaving holes in its blueprint.
    #[serde(default)]
    pub dropped_duplicates: usize,
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
    source_text: Option<&str>,
    // One entry per question this call must produce. Non-empty replaces
    // the automatic per-call Bloom/difficulty quota, so a bank filled in
    // chunks of 5 follows ONE blueprint instead of ten rounded ones.
    slots: &[crate::models::requests::ai::QuestionSlotRequest],
) -> (String, String) {
    let spec = quiz_shape::spec(info.shape);
    // A caller-requested count that would contradict a shape's own
    // fixed-count rule (HighlightWords: exactly one question, scored as
    // a set over the whole transcript) is silently corrected rather than
    // handed to the model as-is — asking for both at once produced zero
    // questions, not either number.
    let count = quiz_shape::fixed_question_count(info.shape).unwrap_or(count);

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
- Di `explanation`, sebut ISI pilihannya, bukan hurufnya (\"karena 0 termasuk bilangan cacah\", bukan \"Pilihan A benar\"): urutan pilihan diacak untuk setiap siswa, jadi huruf yang Anda lihat bukan huruf yang siswa lihat.\n\
- Pengecoh harus mencerminkan kesalahan berpikir yang benar-benar sering terjadi (salah rumus, salah baca data, kesimpulan terlalu jauh), bukan pilihan yang jelas ngawur.\n\
- Jangan membuat soal yang bisa dijawab benar tanpa membaca stimulus.\n\
- Jangan mengulang soal yang sudah ada.\n\
{taxonomy_rules}\n\
- Untuk rumus, notasi matematika, atau simbol kimia, tulis dengan LaTeX di antara $...$.\n\
- JANGAN menggambar diagram (garis bilangan, grafik, bangun datar) dengan LaTeX, termasuk deretan \\text{{---}} atau | di dalam $...$. Huruf LaTeX tidak berjarak sama, sehingga angka tidak jatuh tepat di bawah garisnya dan gambar itu menyesatkan siswa. Nyatakan posisi dengan kata-kata atau tabel. Bila gambar teks sederhana memang perlu, tulis di dalam blok kode ``` pada `stem` (tidak pernah di pilihan jawaban), dengan setiap label tepat di bawah garis tegaknya.\n\
- PENTING: di dalam JSON, setiap garis miring terbalik LaTeX WAJIB ditulis ganda. Tulis \\\\times, \\\\frac, \\\\text — BUKAN \\times, karena JSON akan membacanya sebagai karakter kendali dan rumusnya rusak.\n\
- Nomor soal WAJIB berurutan mulai dari {start_number}.",
        label = info.label,
        description = spec.description,
        schema = spec.schema,
        rules = rules.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n"),
        taxonomy_rules = {
            let level = config.level.as_deref().unwrap_or("");
            // With explicit slots the caller already planned the spread;
            // emitting the automatic quota too would give the model two
            // different targets in the same prompt.
            let base = crate::services::quiz_taxonomy::prompt_rules(level, if slots.is_empty() { count } else { 0 });
            if slots.is_empty() {
                base
            } else {
                let list = slots
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("  Soal ke-{}: bloom {}, kesukaran {}", i + 1, s.bloom, s.difficulty))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{base}\n- SLOT WAJIB — tulis PERSIS {} soal, satu per baris di bawah ini, dengan `taxonomy` yang sama persis:\n{list}", slots.len())
            }
        },
    );

    // Supplying a key for someone else's question is a different job
    // from writing questions, and needs its own instruction rather than
    // a footnote on the writing one.
    if mode == GenerationMode::AnswerOnly {
        let question_json = target.map(|t| serde_json::to_string_pretty(t).unwrap_or_default()).unwrap_or_default();
        let weighted = is_weighted(group);
        let system = format!(
            "Anda adalah penyusun kunci jawaban dan pembahasan untuk soal ujian. Keluarkan HANYA JSON, tanpa prosa, tanpa pagar kode.\n\nBentuk keluaran:\n{shape}\n\nATURAN:\n- JANGAN mengubah pertanyaan, pilihan, atau nomor soal. Anda HANYA menentukan {what}.\n- Pembahasan WAJIB menjelaskan MENGAPA kunci itu benar, dan — bila ada pilihan — mengapa pengecoh yang paling menggoda itu salah.\n- Tulis pembahasan sebagai penjelasan untuk siswa, bukan catatan untuk penulis soal.\n- Di pembahasan, sebut ISI pilihannya, bukan hurufnya — urutan pilihan diacak untuk setiap siswa.\n- Bila soal tidak bisa dijawab dari bacaan/konteks yang diberikan, katakan itu di `explanation` dan tetap berikan jawaban paling masuk akal.\n- Untuk rumus, gunakan LaTeX di antara $...$, dan di dalam JSON tulis garis miring terbalik ganda (\\\\frac, bukan \\frac).\n\nBahasa: {language}.",
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
    } else if let Some(text) = source_text.filter(|s| !s.trim().is_empty()) {
        // "Tempel & Parse" — the author pasted soal they already have
        // (from a PDF, an old worksheet) instead of asking for new ones.
        user.push_str(&format!(
            "EKSTRAK soal dari TEKS di bawah — JANGAN mengarang soal baru. Salin pertanyaan, pilihan, dan (bila terlihat) kunci jawabannya apa adanya, lalu susun ke bentuk JSON di atas. Bila kunci jawaban tidak terlihat di sumber, tentukan sendiri jawaban yang benar dan katakan dasarnya di `explanation`.\n\nTEKS SUMBER:\n{text}\n\n"
        ));
    }
    match mode {
        GenerationMode::Rewrite => {
            let question_json = target.map(|t| serde_json::to_string_pretty(t).unwrap_or_default()).unwrap_or_default();
            let has_instruction = context_prompt.is_some_and(|c| !c.trim().is_empty());
            let directive = if has_instruction {
                // A free-text instruction ("jadikan lebih sulit", "ganti
                // konteks ke olahraga") may deliberately ask to CHANGE the
                // difficulty or topic — telling the model to "keep them
                // the same" in the same breath would just fight the
                // instruction it's about to read below.
                "Tulis ULANG satu soal berikut mengikuti instruksi tambahan di bawah — termasuk bila instruksi itu meminta mengubah tingkat kesulitan, topik, atau konteksnya. Keluarkan tepat 1 soal."
            } else {
                "Tulis ULANG satu soal berikut agar lebih baik — pertahankan topik dan tingkat kesulitannya, perbaiki kejelasan, kualitas pengecoh, dan pembahasannya. Keluarkan tepat 1 soal."
            };
            user.push_str(&format!("{directive}\n\nSOAL LAMA:\n{question_json}\n\n"));
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
        // The reference block already lists the referenced module's
        // sections, numbered — but nothing used to ask for the questions
        // to be SPREAD across them. The pilot showed the cost: a 5-part
        // Modul Belajar got two questions on part 1 and none at all on
        // part 2, because the model writes about whatever it read first.
        if count > 1 {
            user.push_str(
                "\nSEBARAN MATERI: soal harus tersebar ke SELURUH bagian modul rujukan di atas, bukan menumpuk di bagian awal. \
Bila jumlah soal lebih sedikit daripada jumlah bagian, pilih bagian yang paling penting dan sebutkan bagian mana yang diuji di `explanation`. \
Bila soal lebih banyak daripada bagian, barulah satu bagian boleh diuji lebih dari sekali.\n",
            );
        }
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
/// Content words of a stem: LaTeX, numbers and the Indonesian question
/// scaffolding dropped, so "Pada bilangan $4.708$, angka 7 menempati
/// nilai tempat ..." and "Pada bilangan $8.524$, angka 5 menempati nilai
/// tempat ..." read as the near-duplicates they are.
fn stem_words(text: &str) -> std::collections::HashSet<String> {
    const STOP: &[&str] = &[
        "yang", "dan", "dari", "pada", "adalah", "ini", "itu", "dengan", "untuk", "dalam", "sebagai", "berikut", "berapa", "manakah",
        "apakah", "bagaimana", "bilangan", "suatu", "sebuah", "jika", "maka", "tepat", "benar", "paling", "pernyataan", "perhatikan",
        "nilai", "hasil", "adalah", "tersebut", "tentukan", "berikut",
    ];
    let cleaned: String = text.chars().map(|c| if c.is_alphabetic() { c.to_ascii_lowercase() } else { ' ' }).collect();
    cleaned.split_whitespace().filter(|w| w.len() > 2 && !STOP.contains(w)).map(str::to_string).collect()
}

const DUPLICATE_THRESHOLD: f64 = 0.6;

fn is_duplicate_stem(candidate: &str, existing: &[std::collections::HashSet<String>]) -> bool {
    let words = stem_words(candidate);
    if words.len() < 3 {
        return false;
    }
    existing.iter().any(|other| {
        let shared = words.intersection(other).count() as f64;
        let union = words.union(other).count() as f64;
        union > 0.0 && shared / union >= DUPLICATE_THRESHOLD
    })
}

/// Drops generated questions that repeat one the item already has.
/// Filling a 50-question bank in chunks, the model sees earlier stems and
/// is told not to repeat them, but at bank scale it still does — and a
/// near-duplicate saved is a near-duplicate a learner meets twice. The
/// caller is told how many were dropped so it can ask for that many more.
fn drop_duplicates(generated: &mut serde_json::Value, existing_stems: &[std::collections::HashSet<String>]) -> usize {
    let Some(questions) = generated.get_mut("questions").and_then(|q| q.as_array_mut()) else { return 0 };
    let before = questions.len();
    let mut kept_stems: Vec<std::collections::HashSet<String>> = existing_stems.to_vec();
    questions.retain(|q| {
        let stem = q.get("stem").or_else(|| q.get("text")).or_else(|| q.get("prompt")).and_then(|v| v.as_str()).unwrap_or_default();
        if is_duplicate_stem(stem, &kept_stems) {
            return false;
        }
        kept_stems.push(stem_words(stem));
        true
    });
    before - questions.len()
}

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
            // P39-001 — the rewritten question is new content (its stem
            // or answer may be entirely different), so it does NOT keep
            // the old `uid` — `ensure_question_uids` assigns it a fresh
            // one below. But its lineage is worth recording: without
            // this, a question's whole statistical history vanishes the
            // moment an author clicks "Tulis ulang".
            if let Some(old_uid) = target_number
                .and_then(|n| existing.iter().find(|q| value_to_key(q.get("number").unwrap_or(&serde_json::Value::Null)) == n))
                .and_then(|q| q.get("uid"))
                .cloned()
            {
                if let Some(obj) = replacement.as_object_mut() {
                    obj.insert("derived_from_uid".to_string(), old_uid);
                }
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

// --- Not repeating what the rest of the topic already asks ---
//
// A topic's bab each get their own Latihan, generated one at a time,
// and each call used to see only its OWN group's questions. The pilot
// showed the cost: "titik acuan pada garis bilangan disebut ..." was
// asked, near-verbatim, as the C1 recall question of three different
// bab — every bab's module mentions zero as the reference point, and a
// C1 slot pulls the model toward the most quotable fact on the page.
// A bank built that way fills with duplicates a learner meets again and
// again. So a new question now sees every question already written in
// its topic: other groups in this quiz, and every other quiz item in
// the same module.

/// How many sibling questions go into the prompt. A topic in the
/// library has ~7 bab x 5 questions; this leaves room for larger decks
/// without letting one prompt balloon.
const SIBLING_QUESTION_LIMIT: usize = 80;
/// Enough of a stem to recognise what it tests, not the whole stimulus.
const SIBLING_STEM_CHARS: usize = 140;

/// One question written elsewhere in the topic: which quiz it is in, and
/// the start of its stem.
#[derive(Debug, Clone, PartialEq)]
pub struct SiblingQuestion {
    pub source: String,
    pub stem: String,
}

fn clip_stem(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= SIBLING_STEM_CHARS {
        flat
    } else {
        format!("{}…", flat.chars().take(SIBLING_STEM_CHARS).collect::<String>())
    }
}

fn stems_of(source: &str, groups: &[QuizQuestionGroup]) -> Vec<SiblingQuestion> {
    groups
        .iter()
        .flat_map(|g| g.questions.iter())
        .filter_map(|q| q.prompt_text().filter(|t| !t.trim().is_empty()))
        .map(|t| SiblingQuestion { source: source.to_string(), stem: clip_stem(t) })
        .collect()
}

/// Every question in the topic outside the group being generated: the
/// quiz's other groups first (closest to this one), then the module's
/// other quiz items in their tree order.
async fn sibling_questions(pool: &PgPool, item_id: Uuid, quiz: &QuizConfig, group_id: &str) -> Result<Vec<SiblingQuestion>, AppError> {
    let other_groups: Vec<QuizQuestionGroup> = quiz.question_groups.iter().filter(|g| g.group_id != group_id).cloned().collect();
    let mut out = stems_of("grup lain di kuis ini", &other_groups);

    let rows = sqlx::query!(
        r#"select title, quiz_config as "quiz_config!"
           from module_items
           where module_id = (select module_id from module_items where id = $1)
             and id <> $1 and content_type = 'quiz' and quiz_config is not null
           order by order_index asc, created_at asc"#,
        item_id,
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        // A sibling this build cannot parse is skipped, not fatal: an
        // unrelated broken quiz must not block generating this one.
        if let Ok(sibling) = quiz_config_schema::parse(&row.quiz_config) {
            out.extend(stems_of(&row.title, &sibling.question_groups));
        }
    }
    out.truncate(SIBLING_QUESTION_LIMIT);
    Ok(out)
}

/// The prompt section listing them. Framed as "don't test the same
/// thing the same way", not "don't copy this text": a later bab may
/// legitimately build on a concept an earlier one introduced, and a
/// reworded copy of the same recall question is exactly as redundant as
/// a verbatim one.
pub fn sibling_questions_block(siblings: &[SiblingQuestion]) -> String {
    if siblings.is_empty() {
        return String::new();
    }
    let mut block = String::from(
        "\nSOAL YANG SUDAH ADA DI TOPIK INI (di kuis bab lain). JANGAN menguji fakta, istilah, atau kemampuan yang sama dengan cara yang sama, termasuk bila kalimatnya diubah. \
Bila butuh soal C1/C2, pilih fakta yang KHAS bab ini, bukan fakta umum yang juga dibahas bab lain. Konsep bab lain boleh dipakai sebagai bekal, asalkan yang diuji adalah hal baru dari bab ini:\n",
    );
    for s in siblings {
        block.push_str(&format!("- [{}] {}\n", s.source, s.stem));
    }
    block
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
    let target_subtype = bp.convert_to_subtype.as_deref().unwrap_or(&group.r#type);
    let info = quiz_subtype::find(target_subtype).ok_or_else(|| {
        AppError::UnprocessableEntity("unknown_subtype", format!("subtype \"{target_subtype}\" tidak dikenal"))
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
    let reference_block = crate::services::lesson_plan_ai::referenced_items_block(pool, bp.item_id, &reference_ids, bp.reference_section_id.as_deref()).await?;

    let (system_prompt, mut user_prompt) = build_prompt(
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
        bp.raw_text.as_deref(),
        &bp.slots,
    );
    // Only when the model is WRITING questions. Extraction (an image, a
    // pasted worksheet) copies what the source says, and answer_only
    // touches no stem — telling either to avoid the topic's other
    // questions would fight the job they were given.
    let extracting = image_url.is_some() || bp.raw_text.as_deref().is_some_and(|t| !t.trim().is_empty());
    if !extracting && bp.mode != GenerationMode::AnswerOnly {
        let siblings = sibling_questions(pool, bp.item_id, &quiz, &bp.group_id).await?;
        user_prompt.push_str(&sibling_questions_block(&siblings));
    }

    let ai_task_id = Uuid::new_v4();
    let requested = if bp.mode.targets_one_question() { 1 } else { bp.count };
    // 500/question (1500 floor) was too tight in practice — a group that
    // also emits a passage/table/option-pool alongside its questions,
    // in Indonesian, regularly ran past it and got cut off mid-string
    // (surfaced as "Model tidak mengembalikan JSON yang valid"), not
    // because the model failed but because it was truncated. 900/
    // question plus a flat 2000 for that shared context covers real
    // observed output sizes with real headroom to spare; capped well
    // under what gemini-3.8-flash actually supports.
    let settings = crate::services::ai_settings::resolve(pool, config, "quiz_generation").await?;
    let fallback_tokens = settings.max_tokens.map(i64::from).unwrap_or_else(|| ((900 * requested) + 2000).clamp(3000, 32_000));
    let max_tokens = resolve_max_tokens(pool, model, fallback_tokens).await;
    let request = GenerationRequest {
        model: model.to_string(),
        system_prompt,
        user_prompt,
        temperature: settings.temperature.unwrap_or(0.5),
        max_tokens,
        image_url,
        // The whole contract here is a JSON object, so ask for one.
        json_mode: true,
        thinking_budget: Some(QUIZ_THINKING_BUDGET),
        allow_partial: false,
    };

    // One retry, on either a transient provider failure or output that
    // didn't come back as valid JSON — both are exactly the kind of
    // one-off flakiness a second attempt tends to clear, and neither
    // needed the merge below to happen first to detect. Never falls back
    // to a different provider — same model, same provider, one more try
    // — and the LAST failure's real reason is kept so a caller sees
    // something more useful than a bare error code if both attempts fail.
    let mut outcome = None;
    let mut last_error = String::new();
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "quiz group generation provider call failed");
                last_error = format!("Provider gagal: {e}");
                continue;
            }
        };
        match serde_json::from_str::<serde_json::Value>(&strip_code_fence(&generation.text)) {
            Ok(value) => {
                let parsed = repair_latex_escapes(value);
                outcome = Some((parsed, generation));
                break;
            }
            Err(e) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "quiz generation output was not valid JSON");
                last_error = format!("Model tidak mengembalikan JSON yang valid: {e}");
            }
        }
    }
    let Some((mut parsed, generation)) = outcome else {
        tracing::warn!(%ai_task_id, "quiz group generation failed after retry");
        record_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed(Some(last_error)));
    };

    // The merge reads and writes against the FRESHEST row, inside a lock,
    // rather than the `raw_config` read at the top of this function —
    // batch generation runs several groups of the same item concurrently,
    // and a plain "read once, write once" here would let the last writer
    // silently discard every other group's new questions (a lost update),
    // or hand two groups the same question numbers (merge_into_group
    // discards whatever number the model returned and renumbers from
    // `start_number`, so that number has to come from live state too).
    let mut tx = pool.begin().await?;
    let latest_raw_config = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1 for update"#, bp.item_id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten()
        .ok_or(AppError::NotFound("quiz_config_not_found"))?;
    let latest_quiz: QuizConfig = quiz_config_schema::parse(&latest_raw_config)?;
    let fresh_start_number = match bp.mode {
        GenerationMode::Append => start_number_for(&latest_quiz, "").max(start_number_for(&latest_quiz, &bp.group_id)),
        _ => start_number_for(&latest_quiz, &bp.group_id),
    };

    let mut next_config = latest_raw_config.clone();
    let Some(groups) = next_config.get_mut("question_groups").and_then(|g| g.as_array_mut()) else {
        return Err(AppError::Internal(anyhow::anyhow!("quiz_config lost its question_groups")));
    };
    let Some(target) = groups.iter_mut().find(|g| g.get("group_id").and_then(|v| v.as_str()) == Some(bp.group_id.as_str())) else {
        return Err(AppError::NotFound("question_group_not_found"));
    };
    if let Some(new_subtype) = &bp.convert_to_subtype {
        target["type"] = serde_json::json!(new_subtype);
    }
    // Fase 5d — an author-unreviewed group is marked draft so the
    // builder can surface it for approval before submit-review.
    if bp.mark_draft {
        target["ai_meta"] = serde_json::json!({ "draft": true });
    }
    // Scoped to THIS group's own existing questions — that is where a
    // chunked bank actually repeats itself (25 multiple-choice questions
    // written five at a time from the same section). Deliberately not
    // cross-group: the same fact asked as multiple choice and as a short
    // answer is two different exercises, not a duplicate, and comparing
    // across groups would also throw away a whole batch whenever two
    // subtypes happen to phrase a fact the same way.
    let existing_stems: Vec<std::collections::HashSet<String>> = latest_quiz
        .question_groups
        .iter()
        .filter(|g| g.group_id == bp.group_id && bp.mode != GenerationMode::Replace)
        .flat_map(|g| g.questions.iter())
        .filter_map(|q| q.prompt_text().map(stem_words))
        .collect();
    let dropped_duplicates = if bp.mode.targets_one_question() { 0 } else { drop_duplicates(&mut parsed, &existing_stems) };

    let spec = quiz_shape::spec(info.shape);
    let (question_count, emitted) = merge_into_group(
        target,
        &parsed,
        spec.emits,
        fresh_start_number,
        bp.mode,
        bp.question_number.as_deref(),
    );

    // Which section of the Modul Belajar these questions were written
    // from. The model is never asked for it (it has no idea what a
    // section id is) — the caller said which section it referenced, so
    // that is stamped onto exactly the questions this call just added.
    // A bank's draw uses it to spread a paper across the bab's sections,
    // and the filler uses it to know which planned slots are still empty.
    if let Some(section_id) = bp.reference_section_id.as_deref() {
        if let Some(questions) = target.get_mut("questions").and_then(|q| q.as_array_mut()) {
            for question in questions.iter_mut() {
                let number = question.get("number").map(crate::services::quiz_config::value_to_key).unwrap_or_default();
                let is_new = number.split('-').next().and_then(|n| n.trim().parse::<i64>().ok()).is_some_and(|n| n >= fresh_start_number);
                if is_new {
                    question["source_section_id"] = serde_json::json!(section_id);
                }
            }
        }
    }

    if question_count == 0 {
        tracing::warn!(%ai_task_id, raw_output = %generation.text, "quiz generation returned no questions");
        record_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed(Some("Model tidak mengembalikan soal apa pun.".to_string())));
    }

    // P39-001 — the AI never emits `uid` itself (it doesn't know the
    // concept), so every question the merge just wrote or touched gets
    // one assigned here, in the SAME transaction as the save — the only
    // place this write path can do it, since `next_config` is what
    // actually lands in the row below.
    let mut typed_config = quiz_config_schema::parse(&next_config)?;
    crate::services::quiz_config::ensure_question_uids(&mut typed_config);
    crate::services::quiz_config::detect_order_constraints(&mut typed_config);
    next_config = serde_json::to_value(&typed_config).map_err(|e| AppError::Internal(e.into()))?;

    // Refuse to save something the engine could not then dispatch — a
    // duplicate number or a dangling reference would corrupt every
    // attempt on this paper.
    quiz_config_schema::validate_structure(&next_config).map_err(|e| {
        tracing::warn!(error = ?e, %ai_task_id, "quiz generation produced an invalid config");
        e
    })?;

    sqlx::query!(r#"update module_items set quiz_config = $2 where id = $1"#, bp.item_id, next_config)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "quiz_group_generation", PROVIDER, model, PROMPT_ID, generation.tokens_used.map(|t| t as i32)).await?;

    Ok(QuizGenerationResponse { ai_task_id, status: "done", group_id: bp.group_id, question_count, dropped_duplicates, emitted })
}

// POST /ai/quiz/convert-group-type — reshape a group into a different
// subtype, preserving its content instead of inventing new material. A
// thin wrapper around `generate_quiz_group`: it builds a conversion
// instruction out of the group's CURRENT content as `context_prompt`
// (source subtype, existing question stems, passage) and sets
// `convert_to_subtype`, which is what actually makes generate_quiz_group
// build the prompt from the new subtype's shape and write the new type
// onto the group — every other guarantee (retry, row-locked merge,
// real error detail, ai_tasks bookkeeping) comes for free from there.
// Also used to turn a document-import block into a group from scratch —
// the "existing content" is then the block's own extracted text, not a
// prior group's questions (see suggest_group_types).
pub async fn convert_group_type(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    ctx: &AuthContext,
    model: &str,
    item_id: Uuid,
    group_id: String,
    new_subtype: String,
) -> Result<QuizGenerationResponse, AppError> {
    let raw_config = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, item_id)
        .fetch_optional(pool)
        .await?
        .flatten()
        .ok_or(AppError::NotFound("quiz_config_not_found"))?;
    let quiz: QuizConfig = quiz_config_schema::parse(&raw_config)?;
    let group = find_group(&quiz, &group_id).ok_or(AppError::NotFound("question_group_not_found"))?;
    let old_info = quiz_subtype::find(&group.r#type)
        .ok_or_else(|| AppError::UnprocessableEntity("unknown_subtype", format!("subtype \"{}\" tidak dikenal", group.r#type)))?;
    let new_info = quiz_subtype::find(&new_subtype).ok_or_else(|| AppError::UnprocessableEntity("unknown_subtype", format!("subtype \"{new_subtype}\" tidak dikenal")))?;

    let existing: Vec<String> = group.questions.iter().filter_map(|q| q.prompt_text().map(|t| t.chars().take(200).collect::<String>())).collect();
    let mut conversion_brief = format!(
        "KONVERSI TIPE SOAL: ubah grup ini dari \"{}\" menjadi \"{}\". Pertahankan topik dan makna tiap soal — HANYA bentuk/formatnya yang berubah ke tipe baru, JANGAN mengarang materi baru yang tidak berhubungan.\n",
        old_info.label, new_info.label,
    );
    if !existing.is_empty() {
        conversion_brief.push_str(&format!("\nSoal-soal yang harus disesuaikan bentuknya:\n- {}\n", existing.join("\n- ")));
    }
    if let Some(context) = group.context_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
        conversion_brief.push_str(&format!("\nKonteks tambahan dari penulis: {context}\n"));
    }

    let count = group.questions.len().clamp(1, 50) as i64;
    let bp = QuizGenerationBlueprint {
        item_id,
        group_id,
        mode: GenerationMode::Replace,
        question_number: None,
        count,
        context_prompt: Some(conversion_brief),
        reference_module_item_ids: Vec::new(),
        asset_id: None,
        raw_text: None,
        convert_to_subtype: Some(new_subtype),
        slots: Vec::new(),
        reference_section_id: None,
        // Fase 5d — a reshape can lose fidelity; the author reviews it
        // like any other automated fill before it counts as finished.
        mark_draft: true,
    };
    generate_quiz_group(pool, config, ai, storage, ctx, model, bp).await
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct SuggestedBlock {
    pub subtype: String,
    pub count: i64,
    /// The source text this block should be generated FROM — carried
    /// into each block's own `context_prompt` once the author applies
    /// the manifest, so "Generate soal" per group starts already primed
    /// with the right material instead of an empty brief.
    pub excerpt: String,
    pub instruction: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct SuggestedSection {
    pub title: String,
    pub context_prompt: String,
    pub blocks: Vec<SuggestedBlock>,
}

#[derive(Debug, serde::Serialize)]
pub struct SuggestGroupTypesResponse {
    pub ai_task_id: Uuid,
    pub sections: Vec<SuggestedSection>,
}

const SUGGEST_PROMPT_ID: &str = "quiz_suggest_group_types_v1";
/// A document can be huge; this keeps the prompt (and the bill) bounded
/// without truncating so hard the model loses the document's shape.
const SUGGEST_DOCUMENT_CHAR_LIMIT: usize = 20_000;

fn suggest_group_types_system_prompt() -> String {
    let catalog: Vec<String> = quiz_subtype::SUBTYPES
        .iter()
        .filter(|s| !matches!(s.shape, quiz_subtype::SubtypeShape::InteractiveEmbed))
        .map(|s| format!("- {}: {} — {}", s.id, s.label, s.description))
        .collect();
    format!(
        "Anda adalah asisten penyusun struktur kuis untuk platform belajar Indonesia. Baca dokumen yang diberikan dan usulkan struktur bagian + grup soal yang cocok — JANGAN menulis soalnya sendiri, hanya usulkan strukturnya.\n\n\
TIPE SOAL YANG TERSEDIA (pakai HANYA id di sini untuk field \"subtype\"):\n{}\n\n\
Keluarkan HANYA JSON, tanpa prosa, dengan bentuk ini (contoh terisi):\n\
{{\"sections\": [{{\"title\": \"Bacaan 1\", \"context_prompt\": \"Ringkasan singkat isi bagian ini, untuk dipakai AI saat generate soal.\", \"blocks\": [{{\"subtype\": \"multiple_choice\", \"count\": 5, \"excerpt\": \"cuplikan teks sumber untuk blok ini, maks 500 karakter\", \"instruction\": \"Pilih jawaban yang paling tepat.\"}}]}}]}}\n\n\
ATURAN:\n\
- Kelompokkan berdasarkan struktur dokumen yang sebenarnya (bab, bagian, topik) — jangan memaksakan satu bagian untuk seluruh dokumen.\n\
- Pilih tipe soal yang benar-benar cocok dengan bentuk materinya (bacaan panjang → multiple_choice/true_false_not_given, daftar istilah → word_match/vocab_cloze, dan seterusnya).\n\
- `count` wajar untuk panjang materi tiap bagian (jangan mengarang puluhan soal untuk satu paragraf pendek).\n\
- `excerpt` WAJIB berupa kutipan asli dari dokumen, bukan ringkasan karangan.",
        catalog.join("\n"),
    )
}

async fn record_suggest_failed(pool: &PgPool, ai_task_id: Uuid, user_id: Uuid, model: &str) {
    if let Err(e) = ai_task::insert_failed(pool, ai_task_id, user_id, "quiz_suggest_group_types", PROVIDER, model, SUGGEST_PROMPT_ID).await {
        tracing::error!(error = ?e, %ai_task_id, "failed to record failed ai_tasks row");
    }
}

// POST /ai/quiz/suggest-group-types — a document's text in, a proposed
// section + block manifest out. Produces no quiz content itself: the
// author reviews/edits the manifest, then each block becomes a real
// group via `convert_group_type` (used here as "materialize a block
// into a group", the same operation that also reshapes an existing
// group — see that function's doc comment).
pub async fn suggest_group_types(pool: &PgPool, ai: &dyn AIProvider, ctx: &AuthContext, model: &str, document_text: &str) -> Result<SuggestGroupTypesResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let document_text = document_text.trim();
    if document_text.is_empty() {
        return Err(AppError::UnprocessableEntity("document_text_required", "teks dokumen wajib diisi".to_string()));
    }
    let truncated: String = document_text.chars().take(SUGGEST_DOCUMENT_CHAR_LIMIT).collect();

    let ai_task_id = Uuid::new_v4();
    let max_tokens = resolve_max_tokens(pool, model, 6000).await;
    let request = GenerationRequest {
        model: model.to_string(),
        system_prompt: suggest_group_types_system_prompt(),
        user_prompt: format!("DOKUMEN:\n{truncated}\n\nUsulkan struktur bagian dan grup soal yang cocok untuk dokumen ini."),
        temperature: 0.3,
        max_tokens,
        image_url: None,
        json_mode: true,
        thinking_budget: None, allow_partial: false,
    };

    // One retry on the SAME provider — never falls back to a different
    // one. A suggested subtype the registry doesn't recognize is
    // dropped from its block rather than failing the whole manifest —
    // the author still gets a usable structure for everything else.
    let mut outcome = None;
    let mut last_error = String::new();
    for attempt in 0..2 {
        let generation = match ai.generate(request.clone()).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = ?e, %ai_task_id, attempt, "quiz suggest-group-types provider call failed");
                last_error = format!("Provider gagal: {e}");
                continue;
            }
        };
        #[derive(serde::Deserialize)]
        struct Manifest {
            #[serde(default)]
            sections: Vec<SuggestedSection>,
        }
        match serde_json::from_str::<Manifest>(&strip_code_fence(&generation.text)) {
            Ok(manifest) if !manifest.sections.is_empty() => {
                let sections: Vec<SuggestedSection> = manifest
                    .sections
                    .into_iter()
                    .map(|mut s| {
                        s.blocks.retain(|b| quiz_subtype::find(&b.subtype).is_some());
                        s
                    })
                    .filter(|s| !s.blocks.is_empty())
                    .collect();
                if sections.is_empty() {
                    tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "quiz suggest-group-types returned no usable sections after filtering unknown subtypes");
                    last_error = "Model tidak mengusulkan tipe soal yang dikenali.".to_string();
                    continue;
                }
                outcome = Some(sections);
                break;
            }
            Ok(_) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "quiz suggest-group-types returned no sections");
                last_error = "Model tidak mengusulkan bagian apa pun.".to_string();
            }
            Err(e) => {
                tracing::warn!(%ai_task_id, attempt, raw_output = %generation.text, "quiz suggest-group-types output failed to parse");
                last_error = format!("Model tidak mengembalikan JSON yang valid: {e}");
            }
        }
    }
    let Some(sections) = outcome else {
        record_suggest_failed(pool, ai_task_id, ctx.user_id, model).await;
        return Err(AppError::AiOutputValidationFailed(Some(last_error)));
    };

    ai_task::insert_done(pool, ai_task_id, ctx.user_id, "quiz_suggest_group_types", PROVIDER, model, SUGGEST_PROMPT_ID, None).await?;
    Ok(SuggestGroupTypesResponse { ai_task_id, sections })
}

#[derive(Debug, serde::Serialize)]
pub struct BatchGroupResult {
    pub group_id: String,
    pub status: &'static str,
    pub question_count: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct BatchGenerationResponse {
    pub results: Vec<BatchGroupResult>,
}

/// What a client would see from `generate_quiz_group` directly, so a
/// batch failure reads the same as a single-group one — the real
/// message when there is one (e.g. the provider's own error, via
/// `AiOutputValidationFailed`), else the bare error code.
fn error_message(e: &AppError) -> String {
    match e {
        AppError::AiOutputValidationFailed(Some(detail)) => detail.clone(),
        AppError::AiOutputValidationFailed(None) => "ai_output_validation_failed".to_string(),
        AppError::NotFound(code) | AppError::UnprocessableEntity(code, _) | AppError::ForbiddenWithCode(code) => code.to_string(),
        AppError::Forbidden => "forbidden".to_string(),
        _ => "generation_failed".to_string(),
    }
}

// POST /ai/quiz/generate-batch — fills every still-empty group in one
// item. One group's failure (a bad model reply, an unsupported shape)
// must not take the others down with it, so each group's outcome is
// reported individually rather than the whole call erroring out.
pub async fn generate_batch(
    pool: &PgPool,
    config: &Config,
    ai: &dyn AIProvider,
    storage: &dyn AssetStorage,
    ctx: &AuthContext,
    model: &str,
    item_id: Uuid,
    count: i64,
) -> Result<BatchGenerationResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;

    let raw_config = sqlx::query_scalar!(r#"select quiz_config from module_items where id = $1"#, item_id)
        .fetch_optional(pool)
        .await?
        .flatten()
        .ok_or(AppError::NotFound("quiz_config_not_found"))?;
    let quiz: QuizConfig = quiz_config_schema::parse(&raw_config)?;

    // "Still empty" and actually generatable — a hand-authored subtype
    // (H5P, a live speaking activity) has nothing for the model to fill.
    let targets: Vec<String> = quiz
        .question_groups
        .iter()
        .filter(|g| g.questions.is_empty())
        .filter(|g| {
            quiz_subtype::find(&g.r#type).is_some_and(|info| !matches!(info.shape, quiz_subtype::SubtypeShape::InteractiveEmbed))
        })
        .map(|g| g.group_id.clone())
        .collect();

    let results = stream::iter(targets.into_iter().map(|group_id| {
        let bp = QuizGenerationBlueprint {
            item_id,
            group_id: group_id.clone(),
            mode: GenerationMode::Replace,
            question_number: None,
            count,
            context_prompt: None,
            reference_module_item_ids: Vec::new(),
            asset_id: None,
            raw_text: None,
            convert_to_subtype: None,
            slots: Vec::new(),
            reference_section_id: None,
            // Fase 5d — filled without the author looking at any one
            // group individually.
            mark_draft: true,
        };
        async move {
            match generate_quiz_group(pool, config, ai, storage, ctx, model, bp).await {
                Ok(resp) => BatchGroupResult { group_id, status: "done", question_count: Some(resp.question_count), error: None },
                Err(e) => {
                    tracing::warn!(error = ?e, %group_id, %item_id, "batch generation failed for one group");
                    BatchGroupResult { group_id, status: "failed", question_count: None, error: Some(error_message(&e)) }
                }
            }
        }
    }))
    .buffer_unordered(BATCH_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;

    Ok(BatchGenerationResponse { results })
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
    fn rewrite_records_the_old_uid_as_derived_from_uid_and_does_not_keep_it_as_uid() {
        // P39-001 — the rewritten question is new content, so it must
        // NOT keep the old identity as its own `uid` (that would make
        // pre- and post-rewrite statistics indistinguishable), but the
        // lineage back to the question it replaced has to survive
        // somewhere, or a rewrite silently erases a question's history.
        let old_uid = "11111111-1111-1111-1111-111111111111";
        let mut group = json!({"group_id": "g", "type": "multiple_choice", "questions": [
            {"number": 5, "stem": "lama", "uid": old_uid},
        ]});
        let generated = json!({"questions": [{"number": 1, "stem": "tulisan ulang"}]});
        merge_into_group(&mut group, &generated, &[], 7, GenerationMode::Rewrite, Some("5"));
        let rewritten = &group["questions"][0];
        assert_eq!(rewritten["derived_from_uid"], json!(old_uid));
        assert!(rewritten.get("uid").is_none(), "a fresh uid is assigned later by ensure_question_uids, not here");
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
        let (system, user) = build_prompt(&config, group, info, GenerationMode::AnswerOnly, Some(&target), 1, None, 1, false, "", None, &[]);
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
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Rewrite, Some(&target), 1, None, 1, false, "", None, &[]);
        assert!(user.contains("Tulis ULANG"));
        assert!(user.contains("Soal yang kurang jelas"));
        assert!(user.contains("pertahankan topik dan tingkat kesulitannya"), "with no instruction, the model is told to preserve difficulty");
    }

    #[test]
    fn a_rewrite_instruction_is_allowed_to_change_difficulty_instead_of_fighting_it() {
        // Editing one question ("jadikan lebih sulit") reuses Rewrite
        // mode with `context_prompt` as the instruction — the fixed
        // "keep the same difficulty" text must not survive to contradict
        // an instruction that explicitly asks to change it.
        let config = config_with(json!({
            "group_id": "g", "type": "multiple_choice",
            "questions": [{"number": 1, "stem": "Soal mudah"}],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let target = serde_json::to_value(&group.questions[0]).unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Rewrite, Some(&target), 1, Some("jadikan lebih sulit"), 1, false, "", None, &[]);
        assert!(user.contains("Tulis ULANG"));
        assert!(user.contains("mengikuti instruksi tambahan"));
        assert!(!user.contains("pertahankan topik dan tingkat kesulitannya"));
        assert!(user.contains("Panduan penulis: jadikan lebih sulit"));
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
        let (system, _) = build_prompt(&config, group, info, GenerationMode::Replace, None, 1, None, 1, false, "", None, &[]);
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
        let (system, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, Some("Tentang fotosintesis"), 1, false, "", None, &[]);
        assert!(system.contains("explanation"), "prompt must demand explanations");
        // A LaTeX "number line" renders in a proportional font, so its
        // labels drift away from its ticks — found live in the pilot.
        assert!(system.contains("JANGAN menggambar diagram"), "prompt must forbid LaTeX/ASCII diagrams");
        assert!(system.contains("blok kode ```"), "and point at the monospace code block instead");
        assert!(system.contains("mengapa pengecoh"), "prompt must demand distractor analysis");
        assert!(system.contains("Tepat 4 pilihan"), "shape rules must be included");
        assert!(user.contains("Buat 3 soal"));
        assert!(user.contains("Tentang fotosintesis"));
    }

    // Regression — highlight_incorrect_words failed AI generation in
    // every attempt across the Phase 38 QA sweep ("Model tidak
    // mengembalikan soal apa pun"). Root cause: it hard-caps at ONE
    // question (scored as a set over the whole transcript), but every
    // caller here requested 2-4, and the contradiction between "Buat 3
    // soal" and the shape's own "satu grup hanya berisi SATU soal" rule
    // reliably produced zero questions instead of either number.
    #[test]
    fn highlight_words_ignores_the_requested_count_and_always_asks_for_one() {
        let config = config_with(json!({"group_id": "g", "type": "highlight_incorrect_words", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("highlight_incorrect_words").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, "", None, &[]);
        assert!(user.contains("Buat 1 soal"), "user prompt: {user}");
        assert!(!user.contains("Buat 3 soal"));
    }

    #[test]
    fn a_weighted_group_is_told_not_to_write_an_answer_key() {
        let config = config_with(json!({
            "group_id": "g", "type": "multiple_choice",
            "questions": [{"number": 1, "option_scores": {"A": 5}}],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (system, _) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, "", None, &[]);
        assert!(system.contains("option_scores"));
        assert!(system.contains("JANGAN tulis `answer`"));
    }

    #[test]
    fn a_drag_and_drop_group_is_told_to_emit_an_option_pool() {
        let config = config_with(json!({"group_id": "g", "type": "matching", "display_mode": "ielts", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("matching").unwrap();
        let (system, _) = build_prompt(&config, group, info, GenerationMode::Replace, None, 4, None, 1, false, "", None, &[]);
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
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, "", None, &[]);
        assert!(user.contains("Lebah menyerbuki"));
        assert!(!user.contains("Tulis juga bacaannya"));
    }

    #[test]
    fn image_mode_switches_from_inventing_to_extracting() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 5, None, 1, true, "", None, &[]);
        assert!(user.contains("EKSTRAK"), "image mode must tell the model to extract, not invent");
        assert!(user.contains("JANGAN mengarang"));
    }

    #[test]
    fn pasted_raw_text_switches_from_inventing_to_extracting_too() {
        // "Tempel & Parse" — the same extraction instruction as image
        // mode, but the source is text the author pasted rather than an
        // attached photo.
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 5, None, 1, false, "", Some("1. Apa ibu kota Perancis?\nA. Lyon B. Paris"), &[]);
        assert!(user.contains("EKSTRAK"), "pasted-text mode must tell the model to extract, not invent");
        assert!(user.contains("JANGAN mengarang"));
        assert!(user.contains("Apa ibu kota Perancis"), "the pasted source text must actually reach the model");
    }

    #[test]
    fn image_mode_wins_over_pasted_text_when_somehow_both_are_set() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 5, None, 1, true, "", Some("teks yang harusnya diabaikan"), &[]);
        assert!(user.contains("gambar/dokumen terlampir"));
        assert!(!user.contains("teks yang harusnya diabaikan"));
    }

    #[test]
    fn near_duplicate_stems_are_dropped_but_genuinely_different_ones_are_kept() {
        let existing = vec![stem_words("Pada bilangan $4.708$, angka 7 menempati nilai tempat ratusan dan bernilai 700")];
        let mut generated = json!({"questions": [
            {"stem": "Pada bilangan $8.524$, angka 5 menempati nilai tempat ratusan dan bernilai 500"},
            {"stem": "Bulatkan $1.276$ ke ratusan terdekat"},
            {"stem": "Bulatkan $3.849$ ke ratusan terdekat"}
        ]});
        let dropped = drop_duplicates(&mut generated, &existing);
        let kept: Vec<&str> = generated["questions"].as_array().unwrap().iter().map(|q| q["stem"].as_str().unwrap()).collect();
        assert_eq!(dropped, 2, "the place-value twin AND the second rounding twin: {kept:?}");
        assert_eq!(kept, vec!["Bulatkan $1.276$ ke ratusan terdekat"]);
    }

    #[test]
    fn explicit_slots_replace_the_automatic_spread_in_the_prompt() {
        use crate::models::requests::ai::QuestionSlotRequest;
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let slots = vec![
            QuestionSlotRequest { bloom: "c1".into(), difficulty: "mudah".into(), section_id: None },
            QuestionSlotRequest { bloom: "c4".into(), difficulty: "sulit".into(), section_id: None },
        ];
        let (system, _) = build_prompt(&config, group, info, GenerationMode::Replace, None, 2, None, 1, false, "", None, &slots);
        assert!(system.contains("SLOT WAJIB"), "{system}");
        assert!(system.contains("Soal ke-2: bloom c4, kesukaran sulit"));
        assert!(!system.contains("TARGET SEBARAN"), "two different targets in one prompt is what this avoids");
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
            None,
            &[],
        );
        assert!(user.contains("ITEM RUJUKAN"));
        assert!(user.contains("Apa ibu kota Indonesia?"));
    }

    #[test]
    fn an_empty_reference_block_adds_nothing() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, "", None, &[]);
        assert!(!user.contains("ITEM RUJUKAN"));
        assert!(!user.contains("SEBARAN MATERI"), "nothing to spread across when no module is referenced");
    }

    #[test]
    fn a_referenced_module_asks_for_questions_spread_across_its_sections() {
        let config = config_with(json!({"group_id": "g", "type": "multiple_choice", "questions": []}));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let reference = "ITEM RUJUKAN:\n--- @@Pembahasan (article) ---\n  1. Pengertian\n  2. Membaca Lambang\n  3. Nilai Tempat\n";
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 3, None, 1, false, reference, None, &[]);
        assert!(user.contains("SEBARAN MATERI"), "a 5-part module used to get two questions on part 1 and none on part 2: {user}");

        // One rewritten question has nothing to spread — the instruction
        // would just be noise, same reasoning as the taxonomy's own
        // "TARGET SEBARAN" threshold.
        let (_, single) = build_prompt(&config, group, info, GenerationMode::Rewrite, None, 1, None, 1, false, reference, None, &[]);
        assert!(!single.contains("SEBARAN MATERI"));
    }

    #[test]
    fn existing_questions_are_listed_so_they_are_not_repeated() {
        let config = config_with(json!({
            "group_id": "g", "type": "multiple_choice",
            "questions": [{"number": 1, "stem": "Apa ibu kota Indonesia?"}],
        }));
        let group = find_group(&config, "g").unwrap();
        let info = quiz_subtype::find("multiple_choice").unwrap();
        let (_, user) = build_prompt(&config, group, info, GenerationMode::Replace, None, 2, None, 2, false, "", None, &[]);
        assert!(user.contains("jangan diulang"));
        assert!(user.contains("Apa ibu kota Indonesia?"));
    }

    #[test]
    fn sibling_questions_are_listed_with_their_source_and_framed_as_same_skill_not_same_text() {
        let block = sibling_questions_block(&[
            SiblingQuestion { source: "Latihan — Membaca Posisi Bilangan".into(), stem: "Titik pusat pada garis bilangan disebut titik ...".into() },
            SiblingQuestion { source: "Latihan — Lawan Bilangan".into(), stem: "Pasangan bilangan berjarak sama dari nol disebut ...".into() },
        ]);
        assert!(block.contains("- [Latihan — Membaca Posisi Bilangan] Titik pusat pada garis bilangan disebut titik ..."));
        assert!(block.contains("- [Latihan — Lawan Bilangan]"));
        // The pilot's duplicates were reworded, not copied — the rule
        // has to cover a paraphrase too.
        assert!(block.contains("termasuk bila kalimatnya diubah"));
        assert!(block.contains("KHAS bab ini"));
    }

    #[test]
    fn no_siblings_adds_nothing_to_the_prompt() {
        assert_eq!(sibling_questions_block(&[]), "");
    }

    #[test]
    fn a_long_stimulus_is_clipped_to_what_identifies_the_question() {
        let long = format!("Perhatikan data\n\nberikut:   {}", "suhu ".repeat(80));
        let clipped = clip_stem(&long);
        assert!(clipped.starts_with("Perhatikan data berikut: suhu"), "whitespace collapsed: {clipped}");
        assert!(clipped.ends_with('…'));
        assert_eq!(clipped.chars().count(), SIBLING_STEM_CHARS + 1);
        assert_eq!(clip_stem("pendek"), "pendek");
    }

    #[test]
    fn stems_come_from_every_group_and_skip_questions_without_one() {
        let config: QuizConfig = serde_json::from_value(json!({
            "sections": [],
            "question_groups": [
                {"group_id": "g1", "type": "multiple_choice", "questions": [{"number": 1, "stem": "Satu"}, {"number": 2, "answer": "A"}]},
                {"group_id": "g2", "type": "true_false", "questions": [{"number": 3, "text": "Dua"}]},
            ],
        }))
        .unwrap();
        let stems = stems_of("Latihan X", &config.question_groups);
        assert_eq!(stems.iter().map(|s| s.stem.as_str()).collect::<Vec<_>>(), vec!["Satu", "Dua"]);
        assert!(stems.iter().all(|s| s.source == "Latihan X"));
    }

}
