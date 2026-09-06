use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct AssessmentSummary {
    pub id: Uuid,
    pub r#type: String,
    pub title: String,
    pub config: serde_json::Value,
    pub question_count: i64,
}
