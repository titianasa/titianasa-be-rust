use uuid::Uuid;

#[derive(Debug, serde::Deserialize)]
pub struct OpenConversationRequest {
    pub student_id: Option<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
pub struct SendMessageRequest {
    pub body: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateCanvasSessionRequest {
    pub item_id: Uuid,
}
