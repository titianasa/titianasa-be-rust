// Phase 37 — the subtype registry for `quiz`-type module items, ported
// from parelabs' `SubtypeId` design plus its `subtype-meta.ts` metadata
// and `generation/shapes.ts` shape mapping, which used to live in three
// separate files there and are one table here.
//
// Three orthogonal axes, deliberately not collapsed into one:
//
// - `family` is PEDAGOGICAL (reading / listening / grammar / ...). It
//   decides what the AI generator is told to produce.
// - `category` is FORMAT-BASED (pilihan / isian / susun / ...). It's what
//   the author's picker groups by, so a Matematika or Psikotes author is
//   never shown "vocabulary" or "reading" labels for a plain MCQ.
// - `shape` is the AI OUTPUT SCHEMA. ~40 subtypes collapse to 15 shapes,
//   which is why adding a subtype almost never means writing a new
//   prompt or parser — it joins an existing shape.
//
// Same "registry, not enum-per-table" idiom as block_schema.rs and
// question_schema.rs: adding a subtype is one entry here, never a
// migration.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtypeFamily {
    Reading,
    Listening,
    Grammar,
    Vocabulary,
    Production,
    Interactive,
}

/// Subject-neutral, format-based bucket used by the author's picker and
/// the per-group badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtypeCategory {
    /// MCQ-style, auto-graded (one or many choices).
    Pilihan,
    /// Fill / completion / matching.
    Isian,
    /// Reorder / construction.
    Susun,
    /// Audio- or image-driven drill.
    Media,
    /// Open-ended response (essay / record / upload).
    Produksi,
    /// Embedded interactive (H5P, widget).
    Interaktif,
    /// Inventory / Likert scale.
    Skala,
}

/// How a subtype's answer is graded once submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GradingMode {
    /// Deterministic comparison — quiz_score::score_question.
    Auto,
    /// LLM rubric evaluation — ai_writing_evaluation / ai_speaking_evaluation.
    AiRubric,
    /// No automated verdict — lands in the teacher's review queue.
    Manual,
    /// Practice/recall only, no correctness concept (e.g. a flashcard flip).
    SelfCheck,
}

/// Rough cognitive load, for the author's picker (Phase 38) — not a
/// grading concept, purely "how hard is this to answer well" so a
/// non-technical author can tell `true_false` apart from `essay` at a
/// glance without reading every description.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

/// The JSON schema the AI generator must emit for this subtype. Several
/// subtypes share one shape — that sharing is the whole point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtypeShape {
    /// Single-answer A/B/C/D.
    Mcq,
    /// Multi-answer (pick N of M).
    McqMulti,
    /// True/False, optionally with Not Given.
    Tf,
    /// Short text answer, fuzzy-matched.
    SingleText,
    /// gap_fill with inline blanks / IELTS note completion.
    GapFillRich,
    /// table_completion — columns + rows + cells.
    TableFill,
    /// flow_chart — linear steps + options pool.
    FlowFill,
    /// Match a left-hand label to an option from a pool.
    Matching,
    /// Section → heading. Split from `Matching` for prompt clarity.
    MatchingHeadings,
    /// Reorder / scramble (ordered list).
    Sequence,
    /// 5-point rating inventory.
    Likert,
    /// Essay / record / upload — AI- or tutor-evaluated.
    FreeformEval,
    /// H5P or widget embed. Named `InteractiveEmbed` in Rust only to
    /// avoid colliding with `SubtypeFamily::Interactive`; the wire value
    /// stays "interactive", matching parelabs' shape id.
    #[serde(rename = "interactive")]
    InteractiveEmbed,
    /// Click the words in a transcript that differ from the audio.
    HighlightWords,
    /// Live paired-speaking activity card, tutor-scored.
    SpeakingChallenge,
}

/// How a group is PRESENTED, independent of what it tests. The same
/// gap_fill is a plain input in a school worksheet and a drag-and-drop
/// word bank in an IELTS mock — one subtype, two variants, rather than
/// two subtypes that would each need their own scorer and generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayVariant {
    /// Plain inputs / dropdowns.
    Default,
    /// Drag-and-drop from a shared word bank, IELTS-style.
    Ielts,
    /// Checkbox grid with the options as columns — the
    /// "which paragraph contains X? A-G" layout.
    Grid,
}

/// The SHAPE the questions are laid out in. This is what a renderer
/// switches on: several subtypes share one layout (matching and
/// map_labeling are both a pool of options against a list of prompts),
/// which is why ~40 subtypes need a handful of layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    /// One stem, one answer control. The default for most subtypes.
    Flat,
    /// A shared pool of options assigned to labelled prompts.
    WordBank,
    /// A heading pool assigned to passage sections.
    Headings,
    /// Column headers plus rows whose cells may be blanks.
    Table,
    /// A linear sequence of steps, some of them blanks.
    Flow,
    /// Prose with numbered blanks inline (IELTS note/summary completion).
    RichGaps,
    /// A tokenised transcript the learner clicks words in.
    Transcript,
    /// Chips the learner orders into a sentence or word.
    Chips,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SubtypeInfo {
    pub id: &'static str,
    pub family: SubtypeFamily,
    pub category: SubtypeCategory,
    pub grading_mode: GradingMode,
    pub shape: SubtypeShape,
    pub label: &'static str,
    pub description: &'static str,
    pub difficulty: Difficulty,
    /// Unanswerable without a passage — the group, its section, or the
    /// deck must supply one (see quiz_config_schema's resolved_passage).
    pub needs_passage: bool,
    /// Unanswerable without audio — the whole task IS listening to it.
    /// Distinct from `uses_audio`: `short_answer` and `spelling` can be
    /// driven by audio but read perfectly well from a passage or a
    /// written definition, so requiring it would misflag every
    /// reading-based group.
    pub needs_audio: bool,
    /// Can involve audio (drives the author's UI affordances), without
    /// being unanswerable without it. Mirrors parelabs' `audio` flag.
    pub uses_audio: bool,
    /// Drawn from a vocabulary word list rather than a passage.
    pub needs_word_list: bool,
    /// The layout its questions are rendered in.
    pub layout: LayoutKind,
    /// Presentation variants this subtype supports, beyond Default.
    /// Empty = only the default presentation makes sense for it.
    pub variants: &'static [DisplayVariant],
}

use Difficulty::*;
use GradingMode::*;
use SubtypeCategory::*;
use SubtypeFamily::*;
use SubtypeShape::*;

/// Shorthand so each row below reads as data, not as struct boilerplate.
const fn s(
    id: &'static str,
    family: SubtypeFamily,
    category: SubtypeCategory,
    grading_mode: GradingMode,
    shape: SubtypeShape,
    label: &'static str,
    description: &'static str,
) -> SubtypeInfo {
    SubtypeInfo {
        id,
        family,
        category,
        grading_mode,
        shape,
        label,
        description,
        difficulty: Difficulty::Medium,
        needs_passage: false,
        needs_audio: false,
        uses_audio: false,
        needs_word_list: false,
        layout: LayoutKind::Flat,
        variants: &[],
    }
}

/// Give a subtype a non-flat layout, and the presentation variants it
/// supports. `DisplayVariant::Default` is implicit everywhere.
const fn laid_out(mut info: SubtypeInfo, layout: LayoutKind, variants: &'static [DisplayVariant]) -> SubtypeInfo {
    info.layout = layout;
    info.variants = variants;
    info
}

const fn with_passage(mut info: SubtypeInfo) -> SubtypeInfo {
    info.needs_passage = true;
    info
}

/// The task cannot be attempted without audio.
const fn with_audio(mut info: SubtypeInfo) -> SubtypeInfo {
    info.needs_audio = true;
    info.uses_audio = true;
    info
}

/// Audio is supported and common, but the group is still answerable
/// from its passage / written prompt alone.
const fn may_use_audio(mut info: SubtypeInfo) -> SubtypeInfo {
    info.uses_audio = true;
    info
}

const fn with_word_list(mut info: SubtypeInfo) -> SubtypeInfo {
    info.needs_word_list = true;
    info
}

/// Overrides `s()`'s `Medium` default — applied only to the entries
/// that clearly read as easy or hard, so most rows stay unmarked.
const fn difficulty(mut info: SubtypeInfo, d: Difficulty) -> SubtypeInfo {
    info.difficulty = d;
    info
}

pub const SUBTYPES: &[SubtypeInfo] = &[
    // ── Reading — passage-anchored comprehension ──
    difficulty(s("multiple_choice", Reading, Pilihan, Auto, Mcq, "Pilihan Ganda", "Soal pilihan ganda klasik (A/B/C/D), satu jawaban benar."), Easy),
    difficulty(s("multiple_choice_multiple", Reading, Pilihan, Auto, McqMulti, "Pilihan Ganda Kompleks", "Lebih dari satu jawaban benar; siswa harus memilih semuanya. Nilai parsial per pilihan yang tepat."), Hard),
    difficulty(laid_out(s("true_false", Reading, Pilihan, Auto, Tf, "Benar / Salah", "Pernyataan dijawab Benar atau Salah."), LayoutKind::Flat, &[DisplayVariant::Grid]), Easy),
    difficulty(laid_out(s("true_false_not_given", Reading, Pilihan, Auto, Tf, "True / False / Not Given", "Variasi 3-pilihan ala IELTS untuk inferensi dari teks."), LayoutKind::Flat, &[DisplayVariant::Grid]), Easy),
    difficulty(laid_out(s("yes_no_not_given", Reading, Pilihan, Auto, Tf, "Yes / No / Not Given", "Variasi 3-pilihan ala IELTS untuk opini/klaim penulis."), LayoutKind::Flat, &[DisplayVariant::Grid]), Easy),
    laid_out(with_passage(s("gap_fill", Reading, Isian, Auto, GapFillRich, "Gap Fill", "Isi titik-titik dengan kata/frasa dari teks.")), LayoutKind::RichGaps, &[DisplayVariant::Ielts]),
    difficulty(laid_out(with_passage(s("table_completion", Reading, Isian, Auto, TableFill, "Table Completion", "Lengkapi sel-sel di tabel berdasarkan teks bacaan.")), LayoutKind::Table, &[DisplayVariant::Ielts]), Hard),
    laid_out(s("matching", Reading, Isian, Auto, Matching, "Matching", "Cocokkan label dengan opsi dari pool jawaban."), LayoutKind::WordBank, &[DisplayVariant::Ielts, DisplayVariant::Grid]),
    difficulty(laid_out(with_passage(s("matching_headings", Reading, Isian, Auto, MatchingHeadings, "Matching Headings", "Cocokkan tiap paragraf dengan judul yang paling tepat.")), LayoutKind::Headings, &[DisplayVariant::Ielts]), Hard),
    difficulty(laid_out(with_passage(s("flow_chart", Reading, Isian, Auto, FlowFill, "Flow Chart", "Lengkapi step dalam diagram alur dengan opsi yang tersedia.")), LayoutKind::Flow, &[DisplayVariant::Ielts]), Hard),
    laid_out(with_passage(s("map_labeling", Reading, Isian, Auto, Matching, "Map / Diagram Labeling", "Beri label pada elemen di peta atau diagram.")), LayoutKind::WordBank, &[DisplayVariant::Ielts]),
    difficulty(may_use_audio(s("short_answer", Reading, Isian, Auto, SingleText, "Short Answer", "Jawaban singkat 1-3 kata dari passage atau rekaman. Dicocokkan toleran terhadap variasi permukaan.")), Easy),
    // ── Grammar — text manipulation and correction ──
    difficulty(laid_out(with_audio(s("highlight_incorrect_words", Grammar, Pilihan, Auto, HighlightWords, "Highlight Incorrect Words", "Klik kata di transkrip yang berbeda dari audio (ala PTE). Nilai parsial per kata yang tepat ditandai.")), LayoutKind::Transcript, &[]), Hard),
    difficulty(s("error_identification", Grammar, Pilihan, Auto, Mcq, "Error Identification", "Pilih bagian kalimat yang mengandung kesalahan tata bahasa."), Easy),
    difficulty(with_passage(s("cloze_passage", Grammar, Isian, Auto, Mcq, "Cloze Passage", "Paragraf dengan beberapa rumpang inline; tiap rumpang dipilih dari dropdown.")), Hard),
    s("error_correction", Grammar, Isian, Auto, SingleText, "Error Correction", "Kalimat dengan bagian salah ditandai; siswa mengetik koreksi yang tepat."),
    laid_out(s("sentence_reorder", Grammar, Susun, Auto, Sequence, "Sentence Reorder", "Susun chip kata menjadi kalimat yang benar."), LayoutKind::Chips, &[]),
    difficulty(s("word_form", Grammar, Isian, Auto, SingleText, "Word Form", "Kalimat dengan slot kosong plus kata dasar; isi bentuk kata yang tepat."), Easy),
    difficulty(s("sentence_transform", Grammar, Produksi, AiRubric, FreeformEval, "Sentence Transform", "Tulis ulang kalimat memakai kata kunci yang ditentukan. Dinilai AI atau tutor."), Hard),
    // The one open-ended rewrite in this family, so it needs a human's
    // judgment rather than a fixed answer key.
    difficulty(with_passage(s("paragraph_editing", Grammar, Isian, Manual, SingleText, "Paragraph Editing", "Klik kata yang salah di paragraf, lalu tulis koreksinya.")), Hard),
    // ── Vocabulary — recall and matching drills ──
    difficulty(with_word_list(s("flashcard", Vocabulary, Media, SelfCheck, FreeformEval, "Flashcard", "Kartu balik untuk hafalan kosakata, tempo mandiri. Tidak dinilai otomatis.")), Easy),
    difficulty(with_word_list(s("word_match", Vocabulary, Isian, Auto, SingleText, "Word Match", "Cocokkan kata dengan definisinya.")), Easy),
    difficulty(s("vocab_cloze", Vocabulary, Isian, Auto, SingleText, "Vocab Cloze", "Kalimat dengan slot kosong plus petunjuk; isi dengan kata target."), Easy),
    difficulty(may_use_audio(s("spelling", Vocabulary, Isian, Auto, SingleText, "Spelling", "Arti kata plus tombol dengar; siswa mengetik ejaannya.")), Easy),
    laid_out(s("word_scramble", Vocabulary, Susun, Auto, Sequence, "Word Scramble", "Susun chip huruf menjadi kata yang benar."), LayoutKind::Chips, &[]),
    difficulty(s("image_word", Vocabulary, Media, Auto, Mcq, "Image & Word", "Gambar atau deskripsi, lalu pilih kata yang paling sesuai."), Easy),
    s("analogy", Vocabulary, Pilihan, Auto, Mcq, "Analogi", "Pola \"A : B :: C : ?\" — relasi bagian-keseluruhan, fungsi, sebab-akibat, dan sejenisnya."),
    // ── Listening — audio-anchored ──
    with_audio(s("listen_repeat", Listening, Media, AiRubric, FreeformEval, "Listen & Repeat", "Putar pelafalan penutur asli, lalu siswa merekam pengucapannya. Dinilai AI atau tutor.")),
    difficulty(with_audio(s("minimal_pairs", Listening, Media, Auto, Mcq, "Minimal Pairs", "Audio satu kata, lalu pilih mana dari dua kata mirip yang terdengar.")), Easy),
    with_audio(s("stress_pattern", Listening, Media, Auto, Mcq, "Stress Pattern", "Pelafalan kata, lalu pilih suku kata yang ditekan.")),
    with_audio(s("intonation", Listening, Media, AiRubric, FreeformEval, "Intonation", "Pelafalan kalimat, lalu pilih kontur intonasi (naik atau turun).")),
    // ── Production — free-form output ──
    difficulty(s("essay", Production, Produksi, AiRubric, FreeformEval, "Esai / Uraian", "Jawaban tertulis bebas dengan batas kata. Dinilai AI atau tutor."), Hard),
    s("voice_record", Production, Produksi, AiRubric, FreeformEval, "Rekam Suara", "Siswa merekam jawaban audio."),
    difficulty(s("speaking_challenge", Production, Produksi, AiRubric, SpeakingChallenge, "Speaking Challenge", "Aktivitas speaking berpasangan langsung di kelas, dinilai tutor lewat rubrik."), Hard),
    // Media an LLM can't reasonably judge queues for a teacher instead.
    s("video_record", Production, Produksi, Manual, FreeformEval, "Rekam Video", "Siswa merekam jawaban video."),
    s("file_upload", Production, Produksi, Manual, FreeformEval, "Upload Dokumen", "Siswa mengunggah file jawaban (PDF, DOCX, ZIP, dan sejenisnya)."),
    s("image_answer", Production, Produksi, Manual, FreeformEval, "Upload Gambar", "Siswa mengunggah jawaban berupa gambar."),
    // ── Interactive — embedded widgets ──
    difficulty(s("h5p", Interactive, Interaktif, Manual, InteractiveEmbed, "Interaktif H5P", "Sematkan konten interaktif H5P. Skor dihitung di dalam iframe-nya sendiri."), Hard),
    difficulty(s("likert_scale", Interactive, Skala, SelfCheck, Likert, "Skala Likert", "Inventori sikap atau kepribadian. Ditabulasi per dimensi, tidak ada benar/salah."), Easy),
    difficulty(s("widget_interact", Interactive, Interaktif, Manual, InteractiveEmbed, "Widget Interaktif", "Widget menjadi soalnya sendiri — siswa klik, geser, atau pilih untuk menjawab."), Hard),
];

pub fn find(id: &str) -> Option<&'static SubtypeInfo> {
    SUBTYPES.iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_is_unique() {
        let mut ids: Vec<&str> = SUBTYPES.iter().map(|s| s.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate subtype id in registry");
    }

    #[test]
    fn find_is_case_sensitive_and_exact() {
        assert!(find("multiple_choice").is_some());
        assert!(find("Multiple_Choice").is_none());
        assert!(find("not_a_subtype").is_none());
    }

    #[test]
    fn the_registry_still_covers_every_parelabs_subtype() {
        // The port is only faithful if nothing was dropped along the way.
        for id in [
            "multiple_choice", "multiple_choice_multiple", "true_false", "true_false_not_given", "yes_no_not_given",
            "gap_fill", "table_completion", "matching", "matching_headings", "flow_chart", "map_labeling",
            "short_answer", "highlight_incorrect_words", "error_identification", "cloze_passage", "error_correction",
            "sentence_reorder", "word_form", "sentence_transform", "paragraph_editing", "flashcard", "word_match",
            "vocab_cloze", "spelling", "listen_repeat", "minimal_pairs", "word_scramble", "image_word",
            "stress_pattern", "intonation", "essay", "voice_record", "video_record", "file_upload", "image_answer",
            "h5p", "likert_scale", "speaking_challenge", "analogy", "widget_interact",
        ] {
            assert!(find(id).is_some(), "missing subtype {id}");
        }
        assert_eq!(SUBTYPES.len(), 40);
    }

    #[test]
    fn every_auto_subtype_has_a_deterministic_shape() {
        // An Auto subtype whose shape is FreeformEval would be graded
        // against an answer key the generator was never asked to emit.
        for info in SUBTYPES {
            if info.grading_mode == GradingMode::Auto {
                assert_ne!(info.shape, SubtypeShape::FreeformEval, "{} is Auto but emits a freeform shape", info.id);
            }
        }
    }

    #[test]
    fn context_requirements_land_on_the_right_subtypes() {
        assert!(find("matching_headings").unwrap().needs_passage);
        assert!(find("minimal_pairs").unwrap().needs_audio);
        // Dual-use subtypes support audio without requiring it, so a
        // reading-based group is never misflagged as missing a recording.
        assert!(find("short_answer").unwrap().uses_audio);
        assert!(!find("short_answer").unwrap().needs_audio);
        assert!(!find("spelling").unwrap().needs_audio);
        assert!(find("flashcard").unwrap().needs_word_list);
        // A plain MCQ must NOT demand a passage — it's the one subtype
        // reused across every subject, including Matematika.
        assert!(!find("multiple_choice").unwrap().needs_passage);
    }
}
