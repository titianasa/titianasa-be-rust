use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct QuestionBankResponse {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ListQuestionBanksResponse {
    pub items: Vec<QuestionBankResponse>,
}

#[derive(Debug, serde::Serialize)]
pub struct QuestionDetailResponse {
    pub id: Uuid,
    pub bank_id: Uuid,
    pub r#type: String,
    pub difficulty: f64,
    pub data: serde_json::Value,
    pub correct_answer: serde_json::Value,
    pub explanation: Option<serde_json::Value>,
    pub status: String,
    pub qa_report: Option<serde_json::Value>,
    pub cefr_tag: Option<String>,
    pub skill_category: Option<String>,
    pub generated_by: String,
}

#[derive(Debug, serde::Serialize)]
pub struct QuestionStemResponse {
    pub id: Uuid,
    pub r#type: String,
    pub data: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct CreateQuestionResponse {
    pub id: Uuid,
    pub status: String,
}

#[derive(Debug, serde::Serialize)]
pub struct QuestionSummaryResponse {
    pub id: Uuid,
    pub r#type: String,
    pub difficulty: f64,
    pub status: String,
    pub qa_report: Option<serde_json::Value>,
    pub data: serde_json::Value,
    pub skill_category: Option<String>,
    pub generated_by: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ListQuestionsResponse {
    pub items: Vec<QuestionSummaryResponse>,
    pub next_cursor: Option<Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct QuestionStatusResponse {
    pub id: Uuid,
    pub status: String,
    pub qa_report: Option<serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
pub struct CheckAnswerResponse {
    pub correct: bool,
    pub correct_answer: serde_json::Value,
    pub explanation: Option<serde_json::Value>,
    pub rescue_triggered: bool,
    pub rescue_suggested_item_ids: Vec<Uuid>,
}
