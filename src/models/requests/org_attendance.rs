use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct MarkOrgAttendanceRequest {
    pub records: Vec<MarkOrgAttendanceEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub struct MarkOrgAttendanceEntry {
    pub student_id: Uuid,
    pub status: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ScanAttendanceRequest {
    pub token: String,
}
