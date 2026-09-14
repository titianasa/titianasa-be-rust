// Phase 37 — the typed shape of `module_items.quiz_config`, ported
// field-for-field from parelabs' `lib/quiz/types.ts` (KuisUmumConfig /
// KuisUmumSection / QuizQuestionGroup / QuizQuestion).
//
// The field NAMES deliberately match parelabs exactly rather than being
// renamed to local taste: the AI generation shapes/prompts (quiz_shape.rs)
// emit this JSON verbatim, and parelabs' renderers/scorers were written
// against these names. A rename here would turn every future port into a
// translation exercise.
//
// The load-bearing idea, and the one the earlier Titian shape got wrong:
// a question GROUP is not a question. It is a shared context — a reading
// passage, an audio recording, a word list, a diagram — plus an array of
// `questions` that all draw on it. "One IELTS passage, ten questions" is
// one group, not ten. `QuizQuestion.number` (unique across the whole
// deck) is the key the learner's answer map is keyed by.
//
// Everything past `question_groups` is optional: a config mid-authoring
// is expected to be full of holes, and serde `default`s let it round-trip
// without the API rejecting a draft (see quiz_config_schema.rs for the
// split between blocking structure checks and non-blocking QA issues).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// A cell/step/heading entry that may be given either as a bare string or
/// as `{label, text}`. Parelabs' generators emit both forms depending on
/// subtype, and its renderers normalise via `typeof === "string"` — so the
/// Rust side has to accept both rather than force one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LabeledOption {
    Text(String),
    Labeled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default)]
        text: String,
        /// An option can BE a picture — HSK listening matches audio to
        /// one of several images, and figural psikotes items are shapes,
        /// not words. `asset://<id>` or an external url, same as a media
        /// block's `src`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<String>,
    },
}

impl LabeledOption {
    /// The display text, ignoring any label prefix.
    pub fn text(&self) -> &str {
        match self {
            LabeledOption::Text(s) => s,
            LabeledOption::Labeled { text, .. } => text,
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            LabeledOption::Text(_) => None,
            LabeledOption::Labeled { label, .. } => label.as_deref(),
        }
    }

    pub fn image(&self) -> Option<&str> {
        match self {
            LabeledOption::Text(_) => None,
            LabeledOption::Labeled { image, .. } => image.as_deref(),
        }
    }
}

/// Speaker config for TTS-generated listening audio.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuizSpeaker {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
    /// Free-form prompt prefix steering accent/tone, e.g. "Speak with a
    /// calm British accent". Never spoken aloud itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_prompt: Option<String>,
}

/// One entry of a vocabulary-family word list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuizWordItem {
    pub word: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meaning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phonetic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_of_speech: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example_sentence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example_translation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

/// Audio playback settings, shared by both a section and a group (a group
/// inherits its section's when its own slots are empty — see
/// `section_context.rs`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speakers: Vec<QuizSpeaker>,
    /// Max replays allowed. None = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_max_plays: Option<i64>,
    /// Playback rate shown to the learner (0.5–2.0). None = 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_speed: Option<f64>,
    /// "always" | "after_submit" (default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_show_transcript: Option<String>,
    /// Silence after the narrator's intro, giving the learner time to read
    /// the questions before the recording proper starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_reading_pause_seconds: Option<f64>,
    /// Scene/speaker description fed to TTS — never spoken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_context: Option<String>,
    /// "free" (default) hands the learner the browser's own player.
    /// "exam" takes it away: one Play button, no pause, no seeking, no
    /// speed change, no download — how a real listening paper behaves,
    /// where the recording paces the exam and cannot be replayed at will.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_playback_mode: Option<String>,
}

/// One row of a `table_completion` layout. A cell is either static text,
/// a numbered blank, or several lines of interleaved text + blanks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TableRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default)]
    pub cells: Vec<Value>,
}

/// One step of a `flow_chart`. Steps carrying a `question_number` render
/// as an input; the rest are static connectors.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowStep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_number: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

/// A `gap_fill` rich-layout item: static text, or a numbered blank.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GapFillItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_number: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GapFillRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub items: Vec<GapFillItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GapFillSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub rows: Vec<GapFillRow>,
}

/// A lettered passage section, so `matching_headings` can drop headings
/// inline onto the passage at each marker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PassageSection {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_number: Option<Value>,
    #[serde(default)]
    pub text: String,
}

/// A reference card for a live, in-class paired-speaking activity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationCard {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub starter: String,
    #[serde(default)]
    pub follow_up_questions: Vec<String>,
    #[serde(default)]
    pub vocab: Vec<String>,
}

/// A single question. The fields are a shared superset across subtypes;
/// each subtype's renderer and scorer read only the ones it needs. This
/// is deliberately flat rather than an enum-per-subtype: parelabs'
/// generators emit exactly this, and a group can be converted from one
/// subtype to a sibling subtype without a lossy reshape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuizQuestion {
    /// Unique across the WHOLE deck — this is the key the learner's answer
    /// map uses. A string is allowed for multi-mark ranges like "5-6".
    pub number: Value,

    /// P39-001 (ADR-0013 L1) — a permanent identity for this question,
    /// distinct from `number`. `number` is a display label the author
    /// (or a delete-and-renumber) can freely change; every event and
    /// statistic that needs to keep pointing at the SAME question across
    /// reorders, renumbers, and edits keys off `uid` instead. Server-
    /// assigned (`ensure_question_uids`) the moment a question is first
    /// saved — never set by an author or the AI generator directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<Uuid>,
    /// Set only when this question was produced by rewriting another
    /// one (`GenerationMode::Rewrite`) — the `uid` of the question it
    /// replaced. A rewritten question is new content (its stem/answer
    /// may be entirely different), so it gets its OWN `uid` rather than
    /// inheriting the old one; this field is what lets statistics on the
    /// old version be told apart from the new one while still tracing
    /// the lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from_uid: Option<Uuid>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stem: Option<String>,
    /// Alias for `stem` — true_false content is authored with `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Row label for matching / map_labeling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<LabeledOption>,
    /// Per-option point values, keyed by the option's label — the shape
    /// CPNS TKP and BUMN AKHLAK use, where every choice earns something
    /// (1-5) and there is no single "correct" one. Present = the question
    /// is scored by weight instead of right/wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_scores: Option<serde_json::Map<String, Value>>,
    /// The answer key. A bare string, or an array for set-answer subtypes.
    /// Pipe/slash/semicolon-separated alternates are accepted inside a
    /// string ("blue|azure") — see `answer_match::expand_answer_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Value>,
    /// Multi-blank answers for a single gap_fill stem.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_codes: Vec<String>,
    /// Order constraints a shuffled paper must respect (see
    /// `detect_order_constraints`). `choices_fixed`: the choices only
    /// make sense in their authored order and letters — "Semua benar",
    /// "A dan C", an ascending list of numbers. `order_locked`: the stem
    /// leans on the question before it ("berdasarkan soal sebelumnya"),
    /// so the two travel together and keep their order. `None` = not
    /// yet evaluated; an author's explicit `false` is never overwritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices_fixed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_locked: Option<bool>,
    /// The Modul Belajar section (`LessonPlanSection.id`) this question
    /// was written from — lets a draw spread questions across the bab's
    /// sections instead of piling them onto one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_section_id: Option<String>,

    /// Tingkat kesukaran + Bloom C1-C6 (HOTS derived, never stored).
    /// Optional so every question written before this existed still
    /// loads — see quiz_taxonomy.rs for why the two axes stay separate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taxonomy: Option<crate::services::quiz_taxonomy::QuestionTaxonomy>,

    // Grammar-family
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<LabeledOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentence: Option<String>,
    /// The wrong span, appearing verbatim inside `sentence`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_word: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_answer: Option<String>,

    // Vocabulary-family
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meaning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phonetic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example_sentence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_a: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_b: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phonetic_a: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phonetic_b: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scrambled: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub syllables: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stressed_index: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    // Listening-family
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_after_seconds: Option<f64>,
    /// Countdown before the record button unlocks (PTE-style prep time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prep_seconds: Option<f64>,

    // Production-family
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_words: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_words: Option<i64>,
    /// Per-question time budget, used when `timer_mode == "question"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size_mb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_ext: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h5p_url: Option<String>,

    /// Per-question interactive widget, overriding the group's when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<Value>,

    /// Anything an authoring tool or a future subtype adds that this
    /// struct doesn't know about yet survives a round-trip instead of
    /// being silently dropped on the next save.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl QuizQuestion {
    /// The answer-map key for this question — `number` stringified.
    pub fn key(&self) -> String {
        value_to_key(&self.number)
    }

    /// The learner-visible prompt, whichever field carries it.
    pub fn prompt_text(&self) -> Option<&str> {
        self.stem
            .as_deref()
            .or(self.text.as_deref())
            .or(self.prompt.as_deref())
            .or(self.sentence.as_deref())
            .or(self.label.as_deref())
    }

    /// How many answer slots this question occupies. A multi-mark range
    /// like "5-6" is worth 2 points, not 1.
    pub fn slot_count(&self) -> i64 {
        let key = self.key();
        let Some((from, to)) = key.split_once('-') else { return 1 };
        match (from.trim().parse::<i64>(), to.trim().parse::<i64>()) {
            (Ok(a), Ok(b)) if b >= a => b - a + 1,
            _ => 1,
        }
    }
}

/// Stringify a `number` the same way the frontend's `String(q.number)`
/// does, so the two agree on the answer-map key. Serde would render a
/// JSON number as "5.0" via Display on f64; this keeps integers integral.
pub fn value_to_key(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

/// Fields that give the answer away. Everything a renderer needs to SHOW a
/// question stays (stem, choices, word lists, syllables, `error_text` —
/// error_correction highlights it on purpose); only what a scorer or a
/// marker needs is removed.
pub const ANSWER_KEY_FIELDS: &[&str] = &["answer", "answers", "explanation", "option_scores", "model_answer", "rubric", "stressed_index"];

/// The shape a learner may receive. Until this existed `GET /module-items/{id}`
/// sent `quiz_config` verbatim, so every answer key was one network tab
/// away — shuffling choices meant nothing while the key sat in the response.
/// Works on raw JSON rather than `QuizConfig` so fields this crate doesn't
/// model (the struct's `extra` flatten) are stripped by the same rule and
/// can never leak through a round-trip.
pub fn learner_view(raw: &Value) -> Value {
    let mut out = raw.clone();
    if let Some(groups) = out.get_mut("question_groups").and_then(Value::as_array_mut) {
        for group in groups {
            if let Some(questions) = group.get_mut("questions").and_then(Value::as_array_mut) {
                for question in questions.iter_mut().filter_map(Value::as_object_mut) {
                    for field in ANSWER_KEY_FIELDS {
                        question.remove(*field);
                    }
                }
            }
        }
    }
    out
}

fn has_combined_choice(text: &str) -> bool {
    const PHRASES: &[&str] = &[
        "semua benar", "semua jawaban benar", "semua pilihan benar", "semua salah", "semua jawaban salah",
        "tidak ada yang benar", "tidak ada jawaban", "bukan salah satu", "all of the above", "none of the above",
    ];
    let lower = text.to_lowercase();
    if PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    // "A dan C", "(1) dan (3)" style answers refer to OTHER choices by
    // their letter; relettering after a shuffle would silently change
    // what they mean. Short texts only — a real sentence that happens
    // to contain "a" and "dan" is not a cross-reference.
    let tokens: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    if tokens.len() > 8 {
        return false;
    }
    let letters = tokens.iter().filter(|t| matches!(**t, "a" | "b" | "c" | "d" | "e")).count();
    let joined = tokens.iter().any(|t| matches!(*t, "dan" | "and" | "atau" | "or"));
    (letters >= 2 && joined) || tokens.windows(2).any(|w| matches!(w[0], "pilihan" | "jawaban" | "opsi") && matches!(w[1], "a" | "b" | "c" | "d" | "e"))
}

/// A numeral as Indonesian content writes it: "$1.250$", "3,5", "-7".
fn choice_number(text: &str) -> Option<f64> {
    let cleaned: String = text.trim().trim_matches('$').chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() || !cleaned.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-')) {
        return None;
    }
    cleaned.replace('.', "").replace(',', ".").parse().ok()
}

fn choices_are_monotonic_numbers(choices: &[LabeledOption]) -> bool {
    if choices.len() < 3 {
        return false;
    }
    let Some(values) = choices.iter().map(|c| choice_number(c.text())).collect::<Option<Vec<_>>>() else { return false };
    values.windows(2).all(|w| w[0] < w[1]) || values.windows(2).all(|w| w[0] > w[1])
}

fn leans_on_previous_question(stem: &str) -> bool {
    const PHRASES: &[&str] = &[
        "soal sebelumnya", "soal nomor", "nomor sebelumnya", "jawaban sebelumnya", "soal di atas", "berdasarkan jawaban",
        "pertanyaan sebelumnya", "previous question",
    ];
    let lower = stem.to_lowercase();
    PHRASES.iter().any(|p| lower.contains(p))
}

/// Marks the questions a shuffled paper must leave alone. Deterministic
/// and cheap, run on every save (author PATCH and AI merge) next to
/// `ensure_question_uids`. Only ever sets a flag that is still `None`: an
/// author who explicitly un-fixed a question keeps their `false`.
/// Returns whether anything changed.
pub fn detect_order_constraints(config: &mut QuizConfig) -> bool {
    let mut changed = false;
    for group in &mut config.question_groups {
        for (index, question) in group.questions.iter_mut().enumerate() {
            if question.choices_fixed.is_none() && (question.choices.iter().any(|c| has_combined_choice(c.text())) || choices_are_monotonic_numbers(&question.choices)) {
                question.choices_fixed = Some(true);
                changed = true;
            }
            if question.order_locked.is_none() && index > 0 && question.prompt_text().is_some_and(leans_on_previous_question) {
                question.order_locked = Some(true);
                changed = true;
            }
        }
    }
    changed
}

/// P39-001 — gives every question in the deck a stable `uid`: assigns a
/// fresh one to any question that doesn't have one yet, and ALSO to any
/// question whose `uid` collides with one already seen earlier in the
/// same deck (a group or question duplicated client-side before this
/// existed copies the `uid` along with everything else — the second
/// copy needs its own identity, not to share the first's). Returns
/// whether anything changed, so a caller (the backfill binary) can skip
/// writing back a config that was already clean.
///
/// Deliberately does NOT touch `number` or `derived_from_uid` — this is
/// the one function responsible for identity, called from every path
/// that can introduce a question into stored `quiz_config` (an
/// author's save, the AI generator's merge, and the one-time backfill).
pub fn ensure_question_uids(config: &mut QuizConfig) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut changed = false;
    for group in &mut config.question_groups {
        for question in &mut group.questions {
            let needs_new = match question.uid {
                Some(uid) => !seen.insert(uid),
                None => true,
            };
            if needs_new {
                let fresh = Uuid::new_v4();
                question.uid = Some(fresh);
                seen.insert(fresh);
                changed = true;
            }
        }
    }
    changed
}

/// A shared context plus the questions drawn from it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuizQuestionGroup {
    pub group_id: String,
    /// A `quiz_subtype` registry id.
    pub r#type: String,
    /// The instance family. Usually the registry's family, but a template
    /// may override it (TOEFL ITP Structure reuses `multiple_choice` with
    /// family "grammar" so the generator doesn't invent a passage).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,

    /// The section this group belongs to; it inherits that section's
    /// passage/audio when its own are empty. None = free-standing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    /// Sort position among siblings. None = fall back to array order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,

    // ── Shared context ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passage_sections: Vec<PassageSection>,
    #[serde(flatten)]
    pub audio: AudioSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<QuizWordItem>,
    /// Grammar context (paragraph_editing's shared paragraph).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paragraph: Option<String>,
    /// Diagram/map/chart shown above the questions. Required by
    /// map_labeling, useful for flow_chart / table_completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,

    // ── Subtype-specific layout ──
    /// Answer pool for matching / map_labeling / flow_chart /
    /// table_completion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<LabeledOption>,
    /// Heading pool for matching_headings (kept separate from `options`
    /// so the generator prompt can speak about them distinctly).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headings: Vec<LabeledOption>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<TableRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flow: Vec<FlowStep>,
    /// gap_fill's rich note-completion layout (blanks inline in prose).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<GapFillSection>,
    /// multiple_choice_multiple — pick-N limit enforced by the renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_choices: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_description: Option<String>,
    /// highlight_incorrect_words — the displayed transcript, tokenised.
    /// Some tokens deliberately differ from what the audio says.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcript_words: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversation_cards: Vec<ConversationCard>,
    /// "all" (default) | "random" | "choice".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_selection_mode: Option<String>,

    /// "default" | "ielts" (drag-and-drop word bank) | "grid" (matching
    /// only — checkbox grid with options as columns).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_mode: Option<String>,
    /// "ielts" display mode — whether a word-bank option may answer more
    /// than one question ("you may use any letter more than once").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_repeat_options: Option<bool>,

    /// Interactive widget shown above the questions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub widget: Option<Value>,
    /// Inline interactive HTML app, rendered in a sandboxed iframe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codeweb_html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codeweb_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codeweb_height: Option<String>,

    /// Author's free-form guidance, injected into the generator prompt as
    /// `{{context_prompt}}` when this group's "Generate soal" runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_prompt: Option<String>,
    /// Other module items whose content is used as reference context at
    /// generation time. Never shown to the learner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_module_item_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_skill_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_tests: Vec<String>,
    /// Stamped on AI generation, carried forward verbatim for audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_meta: Option<Value>,

    #[serde(default)]
    pub questions: Vec<QuizQuestion>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// A named section grouping related groups under shared media. Sections
/// nest: `parent_section_id` makes the deck a tree, not a flat list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuizSection {
    pub section_id: String,
    /// None = a top-level section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_section_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage: Option<String>,
    #[serde(flatten)]
    pub audio: AudioSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_tests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codeweb_html: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codeweb_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codeweb_height: Option<String>,
    /// Time budget used when `timer_mode == "section"`. At 0 this
    /// section's groups lock read-only while the rest of the quiz runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionPool {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_item_id: Option<Uuid>,
    pub draw_count: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuizConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// "id" | "en" | any other tag the author wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    // ── Timing ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<f64>,
    /// "global" (default) | "section" | "question" | "audio".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timer_mode: Option<String>,
    /// Each listening scope's audio auto-plays and chains to the next,
    /// the way a real listening tape just keeps running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_auto_sequence: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_auto_sequence_gap_seconds: Option<f64>,
    /// `timer_mode == "audio"` only — grace period after the last part
    /// finishes, before auto-submit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_timer_buffer_seconds: Option<f64>,

    // ── Presentation & policy ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passing_score: Option<f64>,
    /// Shuffles the ORDER OF GROUPS within each section (and among
    /// loose groups) — a different group order per learner. Does not
    /// touch the order of questions inside a group, nor answer choices;
    /// see `shuffle_question_order` and `shuffle_choices` for those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffle_questions: Option<bool>,
    /// Shuffles the order QUESTIONS are shown in within a group — only
    /// for groups on the plain "flat" layout (one control per question);
    /// a table/flow/passage-section layout's order is structural, not a
    /// display choice, so this has no effect there. Phase 38 (Fase 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffle_question_order: Option<bool>,
    /// Shuffles the DISPLAY order of each question's answer choices
    /// (multiple_choice and its multi-select sibling) — a different
    /// option order per learner, same correct answer. Safe because each
    /// choice carries its own `label` (see `question-renderers`'
    /// `optionLabel`): only the on-screen position moves, never the
    /// letter that gets submitted and graded. Phase 38 (Fase 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffle_choices: Option<bool>,
    /// false = skip the "Mulai Kuis" guide page. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_guide: Option<bool>,
    /// "default" | "ielts" | "pte". Purely presentational — scoring,
    /// timing and autosave are identical under every theme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Rich (ALM) task instructions shown on the guide page before the
    /// learner starts — "Instruksi Tugas" in the builder. Distinct from
    /// any one group's own `instruction` (plain text, shown inline with
    /// that group's questions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Draw a fresh random subset per attempt instead of showing every
    /// question — "Latihan 10 soal" and "Latihan 25 soal" drawing from
    /// the bab's 50-question bank. `source_item_id` None = draw from this
    /// item's own groups.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_pool: Option<QuestionPool>,
    /// How many times a learner may attempt this quiz. `None`/absent =
    /// unlimited (today's behavior, unchanged). Phase 38.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<i64>,

    // ── Deck-wide default resources (groups inherit when empty) ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passage: Option<String>,
    #[serde(flatten)]
    pub audio: AudioSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub word_list: Vec<QuizWordItem>,

    #[serde(default)]
    pub sections: Vec<QuizSection>,
    #[serde(default)]
    pub question_groups: Vec<QuizQuestionGroup>,

    /// Audit trail of the latest AI generation pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_generation_meta: Option<Value>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl QuizConfig {
    /// Every question across every group, paired with its group.
    pub fn all_questions(&self) -> impl Iterator<Item = (&QuizQuestionGroup, &QuizQuestion)> {
        self.question_groups.iter().flat_map(|g| g.questions.iter().map(move |q| (g, q)))
    }

    pub fn find_section(&self, section_id: &str) -> Option<&QuizSection> {
        self.sections.iter().find(|s| s.section_id == section_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mcq(stem: &str, choices: &[&str]) -> QuizQuestion {
        QuizQuestion {
            number: json!(1),
            stem: Some(stem.into()),
            choices: choices.iter().enumerate().map(|(i, t)| LabeledOption::Labeled { label: Some(((b'A' + i as u8) as char).to_string()), text: (*t).into(), image: None }).collect(),
            ..Default::default()
        }
    }

    fn deck(questions: Vec<QuizQuestion>) -> QuizConfig {
        QuizConfig { question_groups: vec![QuizQuestionGroup { group_id: "g".into(), r#type: "multiple_choice".into(), questions, ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn combined_and_cross_reference_choices_are_fixed() {
        let mut config = deck(vec![
            mcq("Mana yang benar?", &["$0$", "$1$", "Semua benar", "Tidak ada"]),
            mcq("Pernyataan yang tepat?", &["(1) saja", "(2) saja", "A dan B", "B dan C"]),
            mcq("Bilangan cacah terkecil?", &["Nol", "Satu", "Dua", "Tiga"]),
        ]);
        assert!(detect_order_constraints(&mut config));
        let q = &config.question_groups[0].questions;
        assert_eq!(q[0].choices_fixed, Some(true));
        assert_eq!(q[1].choices_fixed, Some(true));
        assert_eq!(q[2].choices_fixed, None, "ordinary word choices stay shuffleable");
    }

    #[test]
    fn ascending_numbers_are_fixed_but_a_scrambled_set_is_not() {
        let mut config = deck(vec![mcq("Berapa?", &["$1.200$", "$1.250$", "$2.000$", "$10.000$"]), mcq("Berapa?", &["$16$", "$28$", "$24$", "$32$"])]);
        detect_order_constraints(&mut config);
        let q = &config.question_groups[0].questions;
        assert_eq!(q[0].choices_fixed, Some(true));
        assert_eq!(q[1].choices_fixed, None);
    }

    #[test]
    fn a_stem_leaning_on_the_previous_question_locks_order_and_an_explicit_false_survives() {
        let mut config = deck(vec![mcq("Hitung $5 + 7$.", &["11", "12"]), mcq("Berdasarkan jawaban soal sebelumnya, kalikan dengan 2.", &["22", "24"])]);
        config.question_groups[0].questions.push(QuizQuestion { order_locked: Some(false), ..mcq("Dari soal nomor 1, berapa?", &["1", "2"]) });
        detect_order_constraints(&mut config);
        let q = &config.question_groups[0].questions;
        assert_eq!(q[0].order_locked, None, "the first question has nothing before it to lean on");
        assert_eq!(q[1].order_locked, Some(true));
        assert_eq!(q[2].order_locked, Some(false), "an author's explicit false is never overwritten");
    }

    #[test]
    fn the_learner_view_drops_every_answer_key_but_keeps_what_renders() {
        let raw = json!({"question_groups": [{"group_id": "g", "type": "multiple_choice", "questions": [
            {"number": 1, "stem": "S", "choices": [{"label": "A", "text": "x"}], "answer": "A", "explanation": "E", "option_scores": {"A": 5}, "taxonomy": {"bloom": "c1"}}
        ]}]});
        let view = learner_view(&raw);
        let q = &view["question_groups"][0]["questions"][0];
        for field in ANSWER_KEY_FIELDS {
            assert!(q.get(*field).is_none(), "{field} leaked");
        }
        assert_eq!(q["stem"], "S");
        assert_eq!(q["choices"][0]["text"], "x");
    }

    #[test]
    fn parses_a_group_of_many_questions_over_one_passage() {
        // The shape the old schema could not express at all: one shared
        // reading passage answered by several questions.
        let config: QuizConfig = serde_json::from_value(json!({
            "sections": [{"section_id": "s1", "title": "Reading Passage 1"}],
            "question_groups": [{
                "group_id": "g1",
                "type": "true_false_not_given",
                "section_id": "s1",
                "passage": "Coral reefs cover less than 1% of the ocean floor.",
                "questions": [
                    {"number": 1, "text": "Coral reefs cover under 1% of the ocean floor.", "answer": "True"},
                    {"number": 2, "text": "Reefs are found only in the Pacific.", "answer": "Not Given"},
                ],
            }],
        }))
        .unwrap();

        assert_eq!(config.question_groups.len(), 1);
        assert_eq!(config.question_groups[0].questions.len(), 2);
        assert_eq!(config.question_groups[0].passage.as_deref().unwrap(), "Coral reefs cover less than 1% of the ocean floor.");
        assert_eq!(config.all_questions().count(), 2);
    }

    #[test]
    fn number_key_matches_the_frontends_string_coercion() {
        let q: QuizQuestion = serde_json::from_value(json!({"number": 7})).unwrap();
        assert_eq!(q.key(), "7");
        let ranged: QuizQuestion = serde_json::from_value(json!({"number": "5-6"})).unwrap();
        assert_eq!(ranged.key(), "5-6");
    }

    #[test]
    fn a_multi_mark_range_is_worth_one_point_per_slot() {
        let ranged: QuizQuestion = serde_json::from_value(json!({"number": "5-7"})).unwrap();
        assert_eq!(ranged.slot_count(), 3);
        let single: QuizQuestion = serde_json::from_value(json!({"number": 5})).unwrap();
        assert_eq!(single.slot_count(), 1);
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        // An authoring tool or a newer subtype may write fields this
        // struct predates; dropping them on the next save would corrupt
        // the author's work silently.
        let original = json!({
            "group_id": "g1",
            "type": "multiple_choice",
            "some_future_field": {"nested": true},
            "questions": [],
        });
        let group: QuizQuestionGroup = serde_json::from_value(original).unwrap();
        let round_tripped = serde_json::to_value(&group).unwrap();
        assert_eq!(round_tripped.get("some_future_field"), Some(&json!({"nested": true})));
    }

    #[test]
    fn audio_settings_flatten_onto_the_group_not_into_a_nested_object() {
        // parelabs emits `audio_url` at the group's top level; a nested
        // `{audio: {...}}` would silently never match.
        let group: QuizQuestionGroup = serde_json::from_value(json!({
            "group_id": "g1",
            "type": "short_answer",
            "audio_url": "https://example.test/a.mp3",
            "audio_max_plays": 2,
            "questions": [],
        }))
        .unwrap();
        assert_eq!(group.audio.audio_url.as_deref(), Some("https://example.test/a.mp3"));
        assert_eq!(group.audio.audio_max_plays, Some(2));
        let back = serde_json::to_value(&group).unwrap();
        assert_eq!(back.get("audio_url").and_then(|v| v.as_str()), Some("https://example.test/a.mp3"));
    }

    #[test]
    fn a_string_or_labeled_option_both_parse() {
        let group: QuizQuestionGroup = serde_json::from_value(json!({
            "group_id": "g1",
            "type": "matching",
            "options": ["plain string", {"label": "A", "text": "labelled"}],
            "questions": [],
        }))
        .unwrap();
        assert_eq!(group.options[0].text(), "plain string");
        assert_eq!(group.options[1].text(), "labelled");
        assert_eq!(group.options[1].label(), Some("A"));
    }

    #[test]
    fn ensure_question_uids_assigns_one_to_every_question_missing_it() {
        let mut config: QuizConfig = serde_json::from_value(json!({
            "sections": [],
            "question_groups": [{
                "group_id": "g1", "type": "multiple_choice",
                "questions": [{"number": 1, "stem": "a"}, {"number": 2, "stem": "b"}],
            }],
        }))
        .unwrap();
        assert!(config.question_groups[0].questions.iter().all(|q| q.uid.is_none()));

        let changed = ensure_question_uids(&mut config);
        assert!(changed);
        let uids: Vec<Uuid> = config.question_groups[0].questions.iter().map(|q| q.uid.unwrap()).collect();
        assert_ne!(uids[0], uids[1], "two different questions must get two different uids");
    }

    #[test]
    fn ensure_question_uids_leaves_an_already_unique_uid_alone() {
        let existing = Uuid::new_v4();
        let mut config: QuizConfig = serde_json::from_value(json!({
            "sections": [],
            "question_groups": [{
                "group_id": "g1", "type": "multiple_choice",
                "questions": [{"number": 1, "stem": "a", "uid": existing}],
            }],
        }))
        .unwrap();

        let changed = ensure_question_uids(&mut config);
        assert!(!changed, "a question that already has a unique uid must not be touched");
        assert_eq!(config.question_groups[0].questions[0].uid, Some(existing));
    }

    #[test]
    fn ensure_question_uids_reassigns_the_second_of_two_duplicated_uids() {
        // The real scenario: a whole group gets duplicated client-side
        // (before the frontend was taught to strip uid on copy) and both
        // copies arrive at the server sharing one uid.
        let dup = Uuid::new_v4();
        let mut config: QuizConfig = serde_json::from_value(json!({
            "sections": [],
            "question_groups": [
                {"group_id": "g1", "type": "multiple_choice", "questions": [{"number": 1, "stem": "original", "uid": dup}]},
                {"group_id": "g2", "type": "multiple_choice", "questions": [{"number": 2, "stem": "copy", "uid": dup}]},
            ],
        }))
        .unwrap();

        let changed = ensure_question_uids(&mut config);
        assert!(changed);
        let first = config.question_groups[0].questions[0].uid.unwrap();
        let second = config.question_groups[1].questions[0].uid.unwrap();
        assert_eq!(first, dup, "the first occurrence keeps the original identity");
        assert_ne!(second, dup, "the second occurrence must not keep sharing it");
    }

    #[test]
    fn ensure_question_uids_never_touches_number_or_derived_from_uid() {
        let lineage = Uuid::new_v4();
        let mut config: QuizConfig = serde_json::from_value(json!({
            "sections": [],
            "question_groups": [{
                "group_id": "g1", "type": "multiple_choice",
                "questions": [{"number": "5-6", "stem": "a", "derived_from_uid": lineage}],
            }],
        }))
        .unwrap();

        ensure_question_uids(&mut config);
        let q = &config.question_groups[0].questions[0];
        assert_eq!(value_to_key(&q.number), "5-6");
        assert_eq!(q.derived_from_uid, Some(lineage));
    }
}
