use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreateProductRequest {
    pub r#type: String,
    pub title: String,
    pub description: Option<String>,
    pub price_idr: i64,
    pub capacity: Option<i32>,
    pub delivery_mode: Option<String>,
    pub session_count: Option<i32>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ListProductsQuery {
    pub cursor: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateCohortRequest {
    pub name: String,
    pub schedule: Option<serde_json::Value>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub meeting_url: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateAssignmentRequest {
    pub title: String,
    pub description: Option<String>,
    pub deadline: Option<DateTime<Utc>>,
}

#[derive(Debug, serde::Deserialize)]
pub struct SubmitAssignmentRequest {
    pub content: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct GradeSubmissionRequest {
    pub score: f64,
    pub feedback: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateReviewRequest {
    pub enrollment_id: Uuid,
    pub rating: i32,
    pub comment: Option<String>,
}
