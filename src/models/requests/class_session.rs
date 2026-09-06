use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreateClassSessionRequest {
    pub session_date: String,
    pub scheduled_start: String,
    pub scheduled_end: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct SimulateParticipantRequest {
    pub role: String,
    pub user_id: Uuid,
    pub join_offset_minutes: i64,
    pub duration_minutes: i64,
}

#[derive(Debug, serde::Deserialize)]
pub struct MarkAttendanceRequest {
    pub records: Vec<MarkAttendanceEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MarkAttendanceEntry {
    pub student_id: Uuid,
    pub status: String,
}
