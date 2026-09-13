use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct EvaluateRequest {
    pub task: String,
    pub input: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct GenerateLessonRequest {
    pub module_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub content_type: String,
    pub topic: String,
    pub grammar_target: Option<String>,
    pub vocab_target: Option<Vec<String>>,
    pub concept_ids: Option<Vec<Uuid>>,
    // Migration 0043 — generate this item under a different subject than
    // its module's.
    pub subject_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct GenerateQuestionsRequest {
    pub bank_id: Uuid,
    pub question_type: String,
    pub topic: String,
    pub count: i64,
    pub difficulty: f64,
    pub concept_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, serde::Deserialize)]
pub struct OcrToQuestionRequest {
    pub bank_id: Uuid,
    pub asset_id: Uuid,
    pub concept_ids: Option<Vec<Uuid>>,
}

/// POST /ai/transcribe-audio — speech-to-text over an asset already in
/// our own Drive. Deliberately asset-only: fetching an arbitrary URL
/// server-side would let a caller aim the server at internal addresses.
#[derive(Debug, serde::Deserialize)]
pub struct TranscribeAudioRequest {
    pub asset_id: Uuid,
}

/// POST /ai/generate-quiz-group — fills one question group in a
/// `quiz_config`. With `asset_id` set the model extracts the questions
/// visible on that image instead of inventing new ones.
#[derive(Debug, serde::Deserialize)]
pub struct GenerateQuizGroupRequest {
    pub item_id: Uuid,
    pub group_id: String,
    /// "replace" (default), "append", "rewrite", or "answer_only".
    pub mode: Option<String>,
    /// Which question `rewrite`/`answer_only` acts on.
    pub question_number: Option<String>,
    #[serde(default = "default_count")]
    pub count: i64,
    pub context_prompt: Option<String>,
    /// Overrides the group's stored `reference_module_item_ids` for this
    /// run only. Omit/empty to use whatever the group already has saved.
    #[serde(default)]
    pub reference_module_item_ids: Vec<Uuid>,
    pub asset_id: Option<Uuid>,
    /// "Tempel & Parse" — pasted text to extract questions from instead
    /// of inventing new ones. Ignored when `asset_id` is set.
    pub raw_text: Option<String>,
    /// Overrides the server's default text model — must be one of
    /// `GET /ai/models`'s ids, or the request is rejected outright
    /// (422 `model_not_allowed`). Ignored when `asset_id` is set: an
    /// image source always needs the vision-capable OCR model, not
    /// whatever text model the author picked.
    pub model: Option<String>,
    /// Fase 5d — stamp the written group `ai_meta.draft = true`. Set by
    /// a caller materializing groups the author hasn't looked at yet
    /// (the document-import dialog's per-block/flat fill); the
    /// interactive "Generate soal" dialog leaves this false, since the
    /// author is looking straight at the result.
    #[serde(default)]
    pub mark_draft: bool,
}

fn default_count() -> i64 {
    1
}

/// POST /ai/quiz/generate-batch — fills every still-empty question
/// group in the item, one call per group, up to 3 at a time.
#[derive(Debug, serde::Deserialize)]
pub struct GenerateQuizBatchRequest {
    pub item_id: Uuid,
    /// Questions per group. Applied uniformly — the batch has no way to
    /// know a subtype-appropriate count per group ahead of time, so the
    /// author picks one number for the whole run.
    #[serde(default = "default_batch_count")]
    pub count: i64,
    pub model: Option<String>,
}

fn default_batch_count() -> i64 {
    5
}

/// POST /ai/quiz/convert-group-type — reshape a group into a different
/// subtype, preserving its content.
#[derive(Debug, serde::Deserialize)]
pub struct ConvertGroupTypeRequest {
    pub item_id: Uuid,
    pub group_id: String,
    pub new_subtype: String,
    pub model: Option<String>,
}

/// POST /ai/quiz/suggest-group-types — a document's text in, a proposed
/// section + block manifest out (no `item_id`: this doesn't touch any
/// stored quiz_config, the author applies the manifest afterward).
#[derive(Debug, serde::Deserialize)]
pub struct SuggestGroupTypesRequest {
    pub document_text: String,
    pub model: Option<String>,
}

/// POST /ai/generate-lesson-plan — the "Buat Otomatis dengan AI" panel.
#[derive(Debug, serde::Deserialize)]
pub struct GenerateLessonPlanRequest {
    pub item_id: Uuid,
    pub topic: String,
    #[serde(default = "default_duration")]
    pub duration_minutes: i64,
    /// Blank lets the model infer it from the topic.
    pub level: Option<String>,
    /// Blank lets the model decide (or read a count out of the notes).
    pub section_count: Option<i64>,
    #[serde(default = "default_language")]
    pub language: String,
    pub notes: Option<String>,
    /// Overrides the server's default text model — must be one of
    /// `GET /ai/models`'s ids, or the request is rejected outright
    /// (422 `model_not_allowed`).
    pub model: Option<String>,
}

/// POST /ai/edit-lesson-section — carries the editor's current, possibly
/// unsaved plan; `referenced_sections` are the `@N` mentions (0-based)
/// and `referenced_item_ids` the `@@item` ones.
#[derive(Debug, serde::Deserialize)]
pub struct EditLessonSectionRequest {
    pub item_id: Uuid,
    pub lesson_plan: serde_json::Value,
    pub section_index: usize,
    pub instruction: String,
    #[serde(default)]
    pub referenced_sections: Vec<usize>,
    #[serde(default)]
    pub referenced_item_ids: Vec<Uuid>,
}

/// POST /ai/translate-lesson-plan
#[derive(Debug, serde::Deserialize)]
pub struct TranslateLessonPlanRequest {
    pub item_id: Uuid,
    pub target_language: String,
}

fn default_duration() -> i64 {
    30
}

fn default_language() -> String {
    "id".to_string()
}
