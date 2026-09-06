use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreateAssessmentRequest {
    pub r#type: String,
    pub title: String,
    pub config: Option<serde_json::Value>,
    pub question_ids: Vec<Uuid>,
}

// POST /attempts/{id}/submit — the 3 fields are mutually exclusive
// depending on the attempt's own kind (assessment vs. writing-lesson vs.
// speaking-lesson), matching assessment_handler.ts's SubmitRequest.
#[derive(Debug, Default, serde::Deserialize)]
pub struct SubmitAttemptRequest {
    pub answers: Option<HashMap<Uuid, serde_json::Value>>,
    pub answer_text: Option<String>,
    pub answer_audio_asset_id: Option<Uuid>,
}
