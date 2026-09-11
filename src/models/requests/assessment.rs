use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreateAssessmentRequest {
    pub r#type: String,
    pub title: String,
    pub config: Option<serde_json::Value>,
    pub question_ids: Vec<Uuid>,
}

// POST /attempts/{id}/submit — mutually exclusive depending on the
// attempt's own kind: `answers` for a plain assessment (question-id
// keyed), `quiz_answers` for a Phase 37 quiz item (question_group-id
// keyed — an arbitrary string from quiz_config, not necessarily a
// UUID).
#[derive(Debug, Default, serde::Deserialize)]
pub struct SubmitAttemptRequest {
    pub answers: Option<HashMap<Uuid, serde_json::Value>>,
    pub quiz_answers: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, serde::Deserialize)]
pub struct GradeManualGroupRequest {
    pub group_id: String,
    /// Which question inside the group this verdict is for — a group can
    /// hold several separately-graded submissions.
    pub question_number: String,
    pub score: f64,
    pub feedback: Option<String>,
}
