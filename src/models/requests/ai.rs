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
