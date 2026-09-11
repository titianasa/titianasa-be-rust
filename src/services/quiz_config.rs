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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffle_questions: Option<bool>,
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
}
