//! Per-question taxonomy — the metadata that says what a question
//! actually demands of the learner, not just what subtype it is.
//!
//! The Indonesian kisi-kisi convention (C1..C6, LOTS/MOTS/HOTS, tingkat
//! kesukaran mudah/sedang/sulit) and the international one (revised
//! Bloom, Anderson & Krathwohl 2001) are not two standards to choose
//! between — they are the same framework under two names. So this
//! models the framework once and carries both labels.
//!
//! Two axes, deliberately NOT collapsed into a single "level":
//!
//!   * `bloom` — the cognitive process: C1 Mengingat .. C6 Mencipta.
//!   * `difficulty` — how hard the item is in practice: mudah/sedang/
//!     sulit.
//!
//! They are independent. A C1 recall question about an obscure date is
//! hard; a C4 analysis question over a two-row table is easy. Storing
//! one number for "level" would lose whichever of the two the author
//! did not mean, and it is exactly the collapse that makes a question
//! bank impossible to balance later.
//!
//! `knowledge_dimension` is the second axis of the same Anderson &
//! Krathwohl table (faktual / konseptual / prosedural / metakognitif),
//! optional because not every subject's authors think in it.
//!
//! LOTS/MOTS/HOTS is **derived** from `bloom` and never stored — see
//! `BloomLevel::thinking_level`. Two fields that can disagree always
//! eventually do; a question marked C5 but LOTS would be unfixable
//! noise in the bank.
//!
//! Room to grow (the explicit ask): unknown keys inside the taxonomy
//! object survive a round-trip in `extra`, and every enum parses
//! leniently, so adding an AKM literacy level, a CEFR band, or Webb's
//! DOK later is an additive change that cannot invalidate questions
//! written today.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

/// Tingkat kesukaran — the empirical hardness of the item, independent
/// of which cognitive process it asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DifficultyLevel {
    Mudah,
    Sedang,
    Sulit,
}

impl DifficultyLevel {
    pub fn parse(raw: &str) -> Option<Self> {
        match normalize(raw).as_str() {
            "mudah" | "easy" | "rendah" | "low" | "1" => Some(Self::Mudah),
            "sedang" | "medium" | "menengah" | "moderate" | "2" => Some(Self::Sedang),
            "sulit" | "hard" | "sukar" | "tinggi" | "high" | "difficult" | "3" => Some(Self::Sulit),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Mudah => "mudah",
            Self::Sedang => "sedang",
            Self::Sulit => "sulit",
        }
    }

    /// The English name an international rubric would use.
    pub fn label_en(self) -> &'static str {
        match self {
            Self::Mudah => "Easy",
            Self::Sedang => "Medium",
            Self::Sulit => "Hard",
        }
    }

    pub fn label_id(self) -> &'static str {
        match self {
            Self::Mudah => "Mudah",
            Self::Sedang => "Sedang",
            Self::Sulit => "Sulit",
        }
    }

    pub const ALL: [Self; 3] = [Self::Mudah, Self::Sedang, Self::Sulit];
}

/// Revised Bloom's cognitive process dimension. The `C1`..`C6` spelling
/// is the Indonesian kisi-kisi convention; the English verbs are the
/// original taxonomy's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BloomLevel {
    C1,
    C2,
    C3,
    C4,
    C5,
    C6,
}

impl BloomLevel {
    pub fn parse(raw: &str) -> Option<Self> {
        match normalize(raw).as_str() {
            "c1" | "1" | "mengingat" | "remember" | "remembering" | "ingatan" => Some(Self::C1),
            "c2" | "2" | "memahami" | "understand" | "understanding" | "pemahaman" => Some(Self::C2),
            "c3" | "3" | "menerapkan" | "apply" | "applying" | "aplikasi" | "penerapan" => Some(Self::C3),
            "c4" | "4" | "menganalisis" | "analyze" | "analyse" | "analyzing" | "analisis" => Some(Self::C4),
            "c5" | "5" | "mengevaluasi" | "evaluate" | "evaluating" | "evaluasi" => Some(Self::C5),
            "c6" | "6" | "mencipta" | "create" | "creating" | "kreasi" | "mengkreasi" => Some(Self::C6),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::C1 => "c1",
            Self::C2 => "c2",
            Self::C3 => "c3",
            Self::C4 => "c4",
            Self::C5 => "c5",
            Self::C6 => "c6",
        }
    }

    /// "C4" — how it is written on an Indonesian kisi-kisi.
    pub fn code(self) -> &'static str {
        match self {
            Self::C1 => "C1",
            Self::C2 => "C2",
            Self::C3 => "C3",
            Self::C4 => "C4",
            Self::C5 => "C5",
            Self::C6 => "C6",
        }
    }

    pub fn label_id(self) -> &'static str {
        match self {
            Self::C1 => "Mengingat",
            Self::C2 => "Memahami",
            Self::C3 => "Menerapkan",
            Self::C4 => "Menganalisis",
            Self::C5 => "Mengevaluasi",
            Self::C6 => "Mencipta",
        }
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Self::C1 => "Remember",
            Self::C2 => "Understand",
            Self::C3 => "Apply",
            Self::C4 => "Analyze",
            Self::C5 => "Evaluate",
            Self::C6 => "Create",
        }
    }

    /// The HOTS mapping Kemendikbud uses, derived rather than stored.
    pub fn thinking_level(self) -> ThinkingLevel {
        match self {
            Self::C1 | Self::C2 => ThinkingLevel::Lots,
            Self::C3 => ThinkingLevel::Mots,
            Self::C4 | Self::C5 | Self::C6 => ThinkingLevel::Hots,
        }
    }

    pub const ALL: [Self; 6] = [Self::C1, Self::C2, Self::C3, Self::C4, Self::C5, Self::C6];
}

/// LOTS / MOTS / HOTS. Always derived from `BloomLevel` — there is no
/// way to set it directly, by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Lots,
    Mots,
    Hots,
}

impl ThinkingLevel {
    pub fn id(self) -> &'static str {
        match self {
            Self::Lots => "lots",
            Self::Mots => "mots",
            Self::Hots => "hots",
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Lots => "LOTS",
            Self::Mots => "MOTS",
            Self::Hots => "HOTS",
        }
    }

    pub fn label_id(self) -> &'static str {
        match self {
            Self::Lots => "Berpikir tingkat rendah",
            Self::Mots => "Berpikir tingkat menengah",
            Self::Hots => "Berpikir tingkat tinggi",
        }
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Self::Lots => "Lower-order thinking",
            Self::Mots => "Middle-order thinking",
            Self::Hots => "Higher-order thinking",
        }
    }

    pub const ALL: [Self; 3] = [Self::Lots, Self::Mots, Self::Hots];
}

/// The knowledge dimension — the other axis of the Anderson & Krathwohl
/// table, and the one Kurikulum Merdeka's dimensi pengetahuan follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeDimension {
    Faktual,
    Konseptual,
    Prosedural,
    Metakognitif,
}

impl KnowledgeDimension {
    pub fn parse(raw: &str) -> Option<Self> {
        match normalize(raw).as_str() {
            "faktual" | "factual" | "fakta" => Some(Self::Faktual),
            "konseptual" | "conceptual" | "konsep" => Some(Self::Konseptual),
            "prosedural" | "procedural" | "prosedur" => Some(Self::Prosedural),
            "metakognitif" | "metacognitive" | "metakognisi" => Some(Self::Metakognitif),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Faktual => "faktual",
            Self::Konseptual => "konseptual",
            Self::Prosedural => "prosedural",
            Self::Metakognitif => "metakognitif",
        }
    }

    pub fn label_id(self) -> &'static str {
        match self {
            Self::Faktual => "Faktual",
            Self::Konseptual => "Konseptual",
            Self::Prosedural => "Prosedural",
            Self::Metakognitif => "Metakognitif",
        }
    }

    pub fn label_en(self) -> &'static str {
        match self {
            Self::Faktual => "Factual",
            Self::Konseptual => "Conceptual",
            Self::Prosedural => "Procedural",
            Self::Metakognitif => "Metacognitive",
        }
    }

    pub const ALL: [Self; 4] = [Self::Faktual, Self::Konseptual, Self::Prosedural, Self::Metakognitif];
}

/// One question's taxonomy block. Every field is optional: questions
/// written before this existed, and questions an author has not
/// classified yet, are valid — an untagged bank degrades to exactly the
/// behaviour it had before, rather than failing to load.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QuestionTaxonomy {
    #[serde(default, deserialize_with = "de_difficulty", skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<DifficultyLevel>,
    #[serde(default, deserialize_with = "de_bloom", skip_serializing_if = "Option::is_none")]
    pub bloom: Option<BloomLevel>,
    #[serde(default, deserialize_with = "de_knowledge", skip_serializing_if = "Option::is_none")]
    pub knowledge_dimension: Option<KnowledgeDimension>,
    /// Anything not modelled here yet — kept verbatim so a future axis
    /// (AKM, CEFR, Webb's DOK, a partner's own scheme) can be written
    /// by one tool and not silently dropped by another.
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

impl QuestionTaxonomy {
    /// Derived, never stored.
    pub fn thinking_level(&self) -> Option<ThinkingLevel> {
        self.bloom.map(BloomLevel::thinking_level)
    }

    pub fn is_empty(&self) -> bool {
        self.difficulty.is_none() && self.bloom.is_none() && self.knowledge_dimension.is_none() && self.extra.is_empty()
    }
}

// --- Lenient parsing ---
//
// The values arrive from a language model, from imported banks, and
// from authors typing by hand, so "C4", "c4", 4, "Menganalisis" and
// "analyze" all have to land on the same level. An unrecognised value
// becomes None instead of failing the whole quiz_config parse: one
// creative answer from the model must not cost an author their entire
// deck. `quiz_config_schema::find_issues` is where a missing
// classification gets surfaced to the human.

fn normalize(raw: &str) -> String {
    raw.trim().to_lowercase().replace(['-', '_', ' '], "")
}

/// Accepts a string ("c4"), a number (4) or null for any of the axes.
fn scalar_token(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn de_difficulty<'de, D: Deserializer<'de>>(d: D) -> Result<Option<DifficultyLevel>, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(scalar_token(&value).and_then(|s| DifficultyLevel::parse(&s)))
}

fn de_bloom<'de, D: Deserializer<'de>>(d: D) -> Result<Option<BloomLevel>, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(scalar_token(&value).and_then(|s| BloomLevel::parse(&s)))
}

fn de_knowledge<'de, D: Deserializer<'de>>(d: D) -> Result<Option<KnowledgeDimension>, D::Error> {
    let value = Value::deserialize(d)?;
    Ok(scalar_token(&value).and_then(|s| KnowledgeDimension::parse(&s)))
}

// --- Vocabulary, for the UI and anything else that needs the labels ---

#[derive(Debug, Serialize)]
pub struct TaxonomyOption {
    pub id: &'static str,
    /// The short code an author writes on a kisi-kisi ("C4", "HOTS").
    /// Absent for axes that have no code of their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    pub label_id: &'static str,
    pub label_en: &'static str,
    /// For bloom levels: which thinking level this maps to, so the UI
    /// never has to hard-code the C1..C6 -> LOTS/MOTS/HOTS table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct TaxonomyVocabulary {
    pub difficulty: Vec<TaxonomyOption>,
    pub bloom: Vec<TaxonomyOption>,
    pub thinking_level: Vec<TaxonomyOption>,
    pub knowledge_dimension: Vec<TaxonomyOption>,
}

pub fn vocabulary() -> TaxonomyVocabulary {
    TaxonomyVocabulary {
        difficulty: DifficultyLevel::ALL
            .iter()
            .map(|d| TaxonomyOption { id: d.id(), code: None, label_id: d.label_id(), label_en: d.label_en(), thinking_level: None })
            .collect(),
        bloom: BloomLevel::ALL
            .iter()
            .map(|b| TaxonomyOption {
                id: b.id(),
                code: Some(b.code()),
                label_id: b.label_id(),
                label_en: b.label_en(),
                thinking_level: Some(b.thinking_level().id()),
            })
            .collect(),
        thinking_level: ThinkingLevel::ALL
            .iter()
            .map(|t| TaxonomyOption { id: t.id(), code: Some(t.code()), label_id: t.label_id(), label_en: t.label_en(), thinking_level: None })
            .collect(),
        knowledge_dimension: KnowledgeDimension::ALL
            .iter()
            .map(|k| TaxonomyOption { id: k.id(), code: None, label_id: k.label_id(), label_en: k.label_en(), thinking_level: None })
            .collect(),
    }
}

// --- Calibration: what each level actually demands ---
//
// A bare "C1..C6" list is not enough for a model to classify with. The
// first pilot (35 questions, Matematika Tahap 1) showed the two failure
// modes this section exists to prevent:
//
//   * Inflation — a two-step lift word problem was tagged C4. It is C3:
//     the learner applies a procedure they were taught. Context and
//     step count make a question LONGER, not higher-order.
//   * Collapse — difficulty just tracked Bloom (every C2 "mudah", every
//     C4 "sulit"), which is the single axis the taxonomy separates.
//
// The fix is the Indonesian kisi-kisi's own tool: Kata Kerja Operasional
// plus a test of what the learner does, and difficulty defined by the
// standard item-analysis index instead of by feel.

impl BloomLevel {
    /// Kata Kerja Operasional and the operational test for the level —
    /// what the learner has to DO, not how the question is worded.
    pub fn operational_definition(self) -> &'static str {
        match self {
            Self::C1 => "menyebutkan, mengenali, mendaftar. Siswa cukup MENGINGAT fakta, istilah, atau definisi persis seperti yang diajarkan. Mengenali definisi yang benar di antara pilihan tetap C1, sepanjang apa pun kalimatnya.",
            Self::C2 => "menjelaskan, menafsirkan, mengklasifikasikan, memberi contoh. Siswa MENANGKAP MAKNA tanpa mengolahnya: membaca posisi pada garis bilangan, tabel, atau grafik; mengubah bentuk representasi; menggolongkan contoh ke konsep.",
            Self::C3 => "menghitung, menggunakan, menerapkan, menyelesaikan. Siswa MENJALANKAN PROSEDUR yang sudah diajarkan, termasuk pada soal cerita dan termasuk bila langkahnya banyak. Soal rutin berkonteks nyata adalah C3, bukan HOTS.",
            Self::C4 => "menguraikan, mendiagnosis, membedakan, menemukan hubungan. Siswa harus menemukan sesuatu yang TIDAK diberitahukan soal: memilih strategi di soal non-rutin, memilah informasi relevan dari yang tidak, menemukan pola, atau menunjukkan letak dan sebab kesalahan dalam suatu penyelesaian.",
            Self::C5 => "menilai, membuktikan, menyanggah, memilih yang terbaik dengan alasan. Siswa MEMBUAT PENILAIAN berdasarkan kriteria: memutuskan apakah suatu klaim atau argumen benar DAN mengapa, atau membandingkan beberapa penyelesaian untuk memilih yang paling tepat. Sekadar mencocokkan pernyataan dengan definisi adalah C2, bukan C5.",
            Self::C6 => "merancang, menyusun, merumuskan. Siswa MENGHASILKAN sesuatu yang baru: menyusun model, strategi, soal, atau contoh yang memenuhi syarat tertentu. Jarang cocok untuk pilihan ganda; bila bentuk soal tidak memungkinkan, ganti dengan C5.",
        }
    }
}

impl DifficultyLevel {
    /// Indeks kesukaran (p) — the proportion of the target learners
    /// expected to answer correctly. Same thresholds item analysis uses,
    /// so a predicted label can later be checked against real answers.
    pub fn operational_definition(self) -> &'static str {
        match self {
            Self::Mudah => "lebih dari 70% siswa sasaran diperkirakan menjawab benar (p > 0,70)",
            Self::Sedang => "30% sampai 70% siswa sasaran diperkirakan menjawab benar (0,30 ≤ p ≤ 0,70)",
            Self::Sulit => "kurang dari 30% siswa sasaran diperkirakan menjawab benar (p < 0,30)",
        }
    }
}

/// Jenjang the questions are written for — decides how much of a paper
/// should be higher-order. Inferred from the free-text `level` a quiz
/// carries, so "Tahap 1 — Matematika Dasar", "SD kelas 5" and "S1" all
/// land somewhere sensible without the author picking from a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Sd,
    Smp,
    Sma,
    Kuliah,
    Pascasarjana,
}

impl Stage {
    pub fn infer(level: &str) -> Option<Self> {
        let l = level.to_lowercase();
        let has_word = |w: &str| l.split(|c: char| !c.is_alphanumeric()).any(|t| t == w);
        // The library's own staging first — it is the most specific.
        for (tahap, stage) in [("1", Self::Sd), ("2", Self::Smp), ("3", Self::Sma), ("4", Self::Kuliah), ("5", Self::Pascasarjana)] {
            if l.contains(&format!("tahap {tahap}")) {
                return Some(stage);
            }
        }
        if l.contains("pascasarjana") || has_word("s2") || has_word("s3") || l.contains("magister") || l.contains("doktor") {
            Some(Self::Pascasarjana)
        } else if has_word("s1") || l.contains("kuliah") || l.contains("universitas") || l.contains("mahasiswa") || l.contains("perguruan tinggi") {
            Some(Self::Kuliah)
        } else if has_word("sma") || has_word("smk") || has_word("ma") || l.contains("utbk") || l.contains("snbt") {
            Some(Self::Sma)
        } else if has_word("smp") || has_word("mts") {
            Some(Self::Smp)
        } else if has_word("sd") || has_word("mi") {
            Some(Self::Sd)
        } else {
            None
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sd => "SD/MI",
            Self::Smp => "SMP/MTs",
            Self::Sma => "SMA/SMK/MA",
            Self::Kuliah => "perguruan tinggi (S1)",
            Self::Pascasarjana => "pascasarjana",
        }
    }

    /// Target share of C1..C6, in percent. HOTS (C4+) rises with the
    /// stage: ~15% for SD, where most of what is new is still the
    /// procedure itself, up to ~75% for postgraduate work.
    fn bloom_weights(stage: Option<Self>) -> [u32; 6] {
        match stage {
            Some(Self::Sd) => [20, 35, 30, 15, 0, 0],
            Some(Self::Smp) => [10, 25, 35, 20, 10, 0],
            Some(Self::Sma) => [5, 20, 35, 25, 10, 5],
            Some(Self::Kuliah) => [0, 15, 30, 30, 15, 10],
            Some(Self::Pascasarjana) => [0, 5, 20, 35, 25, 15],
            // Unknown jenjang: a middle-of-the-road paper.
            None => [10, 25, 35, 20, 10, 0],
        }
    }
}

/// mudah : sedang : sulit = 3 : 5 : 2, the usual proportion for a
/// balanced paper. Applies at every stage — difficulty is relative to
/// the target learners, so an SD paper and an S1 paper both need their
/// easy and hard items.
const DIFFICULTY_WEIGHTS: [u32; 3] = [30, 50, 20];

/// Splits `count` questions across `weights` by largest remainder, so
/// the counts always add up exactly and a zero weight always gets zero.
/// Ties go to the earlier (lower) level.
fn apportion(weights: &[u32], count: i64) -> Vec<i64> {
    let count = count.max(0);
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return vec![0; weights.len()];
    }
    let exact: Vec<f64> = weights.iter().map(|&w| w as f64 * count as f64 / total as f64).collect();
    let mut out: Vec<i64> = exact.iter().map(|e| e.floor() as i64).collect();
    let mut leftover = count - out.iter().sum::<i64>();
    let mut order: Vec<usize> = (0..weights.len()).filter(|&i| weights[i] > 0).collect();
    order.sort_by(|&a, &b| (exact[b] - exact[b].floor()).partial_cmp(&(exact[a] - exact[a].floor())).unwrap().then(a.cmp(&b)));
    for i in order.into_iter().cycle() {
        if leftover == 0 {
            break;
        }
        out[i] += 1;
        leftover -= 1;
    }
    out
}

/// The target spread for one generation, as concrete counts.
pub fn target_spread(level: &str, count: i64) -> (Option<Stage>, Vec<(BloomLevel, i64)>, Vec<(DifficultyLevel, i64)>) {
    let stage = Stage::infer(level);
    let bloom = BloomLevel::ALL.into_iter().zip(apportion(&Stage::bloom_weights(stage), count)).filter(|(_, n)| *n > 0).collect();
    let difficulty = DifficultyLevel::ALL.into_iter().zip(apportion(&DIFFICULTY_WEIGHTS, count)).filter(|(_, n)| *n > 0).collect();
    (stage, bloom, difficulty)
}

/// The taxonomy block, spelled out for the generator prompt. Kept next
/// to the enums so a new level can never be added to one without the
/// model being told about it.
pub fn prompt_rules(level: &str, count: i64) -> String {
    let definitions = BloomLevel::ALL
        .iter()
        .map(|b| format!("  {} {} ({}): {}", b.code(), b.label_id(), b.thinking_level().code(), b.operational_definition()))
        .collect::<Vec<_>>()
        .join("\n");
    let difficulty = DifficultyLevel::ALL
        .iter()
        .map(|d| format!("  {}: {}", d.id(), d.operational_definition()))
        .collect::<Vec<_>>()
        .join("\n");

    let (stage, bloom_target, difficulty_target) = target_spread(level, count);
    let jenjang = stage.map(Stage::label).unwrap_or("jenjang tidak disebutkan — anggap menengah");
    let bloom_target = bloom_target.iter().map(|(b, n)| format!("{n} soal {}", b.code())).collect::<Vec<_>>().join(", ");
    let difficulty_target = difficulty_target.iter().map(|(d, n)| format!("{n} {}", d.id())).collect::<Vec<_>>().join(", ");
    // Only a group of several questions has a spread to aim for; asking
    // one rewritten question to be "1 soal C2, ..." is noise.
    let spread = if count >= 3 {
        format!(
            "\n- TARGET SEBARAN untuk {count} soal ini (jenjang: {jenjang}):\n  Bloom: {bloom_target}.\n  Kesukaran: {difficulty_target}.\n  \
Rencanakan sebaran ini SEBELUM menulis, lalu tulis soal yang BENAR-BENAR menuntut level itu. Jangan menulis soal dulu lalu menempelkan label agar kuotanya terpenuhi. Bila materi tidak memungkinkan suatu level, geser ke level terdekat dan beri label yang jujur; label yang benar lebih penting daripada kuota yang pas."
        )
    } else {
        String::new()
    };

    format!(
        "- Setiap soal WAJIB punya `taxonomy` berisi `difficulty` (\"mudah\" | \"sedang\" | \"sulit\") dan `bloom` (\"c1\"..\"c6\"). JANGAN menulis LOTS/MOTS/HOTS — itu diturunkan otomatis dari `bloom`.\n\
- Tentukan `bloom` dari apa yang DILAKUKAN siswa untuk sampai ke jawaban, bukan dari panjang atau konteks soalnya:\n{definitions}\n\
- Dua kesalahan pelabelan yang paling sering, hindari:\n  \
(a) Soal cerita atau soal banyak langkah dilabeli C4. Bila siswa hanya menjalankan prosedur yang sudah diajarkan, itu C3, serumit apa pun ceritanya.\n  \
(b) Soal yang jawabannya bisa disalin atau dikenali langsung dari bacaan dilabeli C2 ke atas. Itu C1.\n\
- Tentukan `difficulty` SENDIRI, terpisah dari `bloom`, dengan memperkirakan berapa persen siswa sasaran yang akan menjawab benar:\n{difficulty}\n  \
Keduanya sering tidak searah: soal C1 tentang istilah yang jarang dipakai bisa sulit, soal C4 atas data dua baris bisa mudah. Jangan menyamakan C1–C2 dengan mudah dan C4 ke atas dengan sulit.{spread}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hots_is_derived_from_bloom_not_stored() {
        assert_eq!(BloomLevel::C2.thinking_level(), ThinkingLevel::Lots);
        assert_eq!(BloomLevel::C3.thinking_level(), ThinkingLevel::Mots);
        assert_eq!(BloomLevel::C4.thinking_level(), ThinkingLevel::Hots);
        assert_eq!(BloomLevel::C6.thinking_level(), ThinkingLevel::Hots);
    }

    #[test]
    fn the_same_level_is_reachable_by_indonesian_english_code_and_number() {
        for raw in ["C4", "c4", "4", "menganalisis", "Analyze", " analisis "] {
            assert_eq!(BloomLevel::parse(raw), Some(BloomLevel::C4), "failed on {raw:?}");
        }
        for raw in ["sulit", "Hard", "sukar", "3"] {
            assert_eq!(DifficultyLevel::parse(raw), Some(DifficultyLevel::Sulit), "failed on {raw:?}");
        }
        assert_eq!(KnowledgeDimension::parse("Procedural"), Some(KnowledgeDimension::Prosedural));
    }

    #[test]
    fn an_unrecognised_value_degrades_to_none_instead_of_failing_the_parse() {
        // A model that invents "very hard" must cost that one field,
        // not the author's whole deck.
        let t: QuestionTaxonomy = serde_json::from_value(serde_json::json!({
            "difficulty": "very hard",
            "bloom": "c5",
        }))
        .expect("taxonomy with a bad value still parses");
        assert_eq!(t.difficulty, None);
        assert_eq!(t.bloom, Some(BloomLevel::C5));
    }

    #[test]
    fn a_number_is_accepted_for_either_axis() {
        let t: QuestionTaxonomy = serde_json::from_value(serde_json::json!({"difficulty": 2, "bloom": 6})).unwrap();
        assert_eq!(t.difficulty, Some(DifficultyLevel::Sedang));
        assert_eq!(t.bloom, Some(BloomLevel::C6));
    }

    #[test]
    fn an_axis_we_have_not_modelled_yet_survives_a_round_trip() {
        // The extensibility promise: a future AKM/CEFR/DOK key written
        // by one tool must not be erased by another tool saving the
        // same question.
        let raw = serde_json::json!({"bloom": "c3", "akm_level": "L2", "dok": 3});
        let t: QuestionTaxonomy = serde_json::from_value(raw).unwrap();
        let back = serde_json::to_value(&t).unwrap();
        assert_eq!(back["akm_level"], serde_json::json!("L2"));
        assert_eq!(back["dok"], serde_json::json!(3));
        assert_eq!(back["bloom"], serde_json::json!("c3"));
    }

    #[test]
    fn an_empty_taxonomy_serializes_to_nothing() {
        let t = QuestionTaxonomy::default();
        assert!(t.is_empty());
        assert_eq!(serde_json::to_value(&t).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn the_prompt_names_every_bloom_level_and_forbids_writing_hots_directly() {
        let rules = prompt_rules("", 5);
        for b in BloomLevel::ALL {
            assert!(rules.contains(b.code()), "prompt must name {}", b.code());
        }
        assert!(rules.contains("JANGAN menulis LOTS/MOTS/HOTS"));
    }

    #[test]
    fn the_library_stage_and_school_names_both_resolve_to_a_jenjang() {
        assert_eq!(Stage::infer("Tahap 1 — Matematika Dasar"), Some(Stage::Sd));
        assert_eq!(Stage::infer("Tahap 4 — Kalkulus"), Some(Stage::Kuliah));
        assert_eq!(Stage::infer("SD kelas 5"), Some(Stage::Sd));
        assert_eq!(Stage::infer("SMP/MTs"), Some(Stage::Smp));
        assert_eq!(Stage::infer("Persiapan UTBK"), Some(Stage::Sma));
        assert_eq!(Stage::infer("mahasiswa S1"), Some(Stage::Kuliah));
        assert_eq!(Stage::infer("Magister manajemen"), Some(Stage::Pascasarjana));
        assert_eq!(Stage::infer("sesuai jenjang materi"), None);
        // A word that merely CONTAINS "sd"/"ma" must not match — "masa"
        // is not Madrasah Aliyah.
        assert_eq!(Stage::infer("materi masa kini"), None);
    }

    #[test]
    fn a_target_spread_always_adds_up_to_the_requested_count() {
        for level in ["Tahap 1", "Tahap 3", "Tahap 5", "SMP", "", "S2"] {
            for count in [1, 3, 5, 7, 10, 20, 50] {
                let (_, bloom, difficulty) = target_spread(level, count);
                assert_eq!(bloom.iter().map(|(_, n)| n).sum::<i64>(), count, "bloom {level:?} x{count}");
                assert_eq!(difficulty.iter().map(|(_, n)| n).sum::<i64>(), count, "difficulty {level:?} x{count}");
            }
        }
    }

    #[test]
    fn higher_order_share_rises_with_the_jenjang() {
        let hots = |level: &str| {
            let (_, bloom, _) = target_spread(level, 20);
            bloom.iter().filter(|(b, _)| b.thinking_level() == ThinkingLevel::Hots).map(|(_, n)| n).sum::<i64>()
        };
        let sd = hots("Tahap 1");
        let smp = hots("Tahap 2");
        let sma = hots("Tahap 3");
        let s1 = hots("Tahap 4");
        let s2 = hots("Tahap 5");
        assert!(sd < smp && smp < sma && sma < s1 && s1 < s2, "{sd} {smp} {sma} {s1} {s2}");
        // SD: some HOTS, but not a third of the paper as the pilot had.
        assert!((1..=4).contains(&sd), "SD HOTS out of 20: {sd}");
    }

    #[test]
    fn a_five_question_sd_group_gets_one_hots_question_and_no_c5_or_c6() {
        let (stage, bloom, difficulty) = target_spread("Tahap 1 — Matematika Dasar", 5);
        assert_eq!(stage, Some(Stage::Sd));
        assert_eq!(bloom, vec![(BloomLevel::C1, 1), (BloomLevel::C2, 2), (BloomLevel::C3, 1), (BloomLevel::C4, 1)]);
        assert_eq!(difficulty, vec![(DifficultyLevel::Mudah, 2), (DifficultyLevel::Sedang, 2), (DifficultyLevel::Sulit, 1)]);
    }

    #[test]
    fn the_prompt_names_the_two_known_mislabels_and_the_spread_only_for_groups() {
        let group = prompt_rules("Tahap 1", 5);
        assert!(group.contains("Soal cerita atau soal banyak langkah dilabeli C4"));
        assert!(group.contains("p > 0,70"));
        assert!(group.contains("TARGET SEBARAN untuk 5 soal ini (jenjang: SD/MI)"));
        assert!(group.contains("1 soal C4"));
        // One rewritten question has no spread to aim for.
        assert!(!prompt_rules("Tahap 1", 1).contains("TARGET SEBARAN"));
    }

}
