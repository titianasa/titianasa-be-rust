use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct CreatePolicyRequest {
    pub exam_type: String,
    pub camera: Option<String>,
    pub microphone: Option<String>,
    pub screen: Option<String>,
    pub fullscreen_required: Option<bool>,
    pub focus_monitoring: Option<bool>,
    pub retention_days: Option<i32>,
}

#[derive(Debug, serde::Deserialize)]
pub struct StartSessionRequest {
    pub policy_id: Uuid,
    pub consent: bool,
    pub device_info: Option<serde_json::Value>,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateEventRequest {
    pub r#type: String,
    pub severity: String,
    pub metadata: Option<serde_json::Value>,
    pub evidence_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReviewRequest {
    pub decision: String,
    pub notes: Option<String>,
}
