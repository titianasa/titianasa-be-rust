#[derive(Debug, serde::Deserialize)]
pub struct CreateOrgClassSessionRequest {
    pub session_date: String,
    pub scheduled_start: String,
    pub scheduled_end: String,
}
