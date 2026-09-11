use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct ClassSummaryResponse {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub teacher_id: Uuid,
    pub teacher_name: String,
    pub module_id: Option<Uuid>,
    pub module_title: Option<String>,
    pub program_id: Option<Uuid>,
    pub program_title: Option<String>,
    pub period_id: Option<Uuid>,
    pub period_name: Option<String>,
    pub member_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct ClassListResponse {
    pub items: Vec<ClassSummaryResponse>,
}

#[derive(Debug, serde::Serialize)]
pub struct ClassMemberResponse {
    pub student_id: Uuid,
    pub name: String,
    pub email: String,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize)]
pub struct ClassDetailResponse {
    #[serde(flatten)]
    pub summary: ClassSummaryResponse,
    pub members: Vec<ClassMemberResponse>,
}
