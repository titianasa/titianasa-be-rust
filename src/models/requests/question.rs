use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreateQuestionBankRequest {
    pub subject_id: Uuid,
    pub name: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateQuestionRequest {
    pub r#type: String,
    pub difficulty: f64,
    pub data: serde_json::Value,
    pub correct_answer: serde_json::Value,
    pub explanation: Option<serde_json::Value>,
    pub concept_ids: Option<Vec<Uuid>>,
    pub skill_category: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CheckAnswerRequest {
    pub submitted_answer: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct ListQuestionsQuery {
    pub status: Option<String>,
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
}
