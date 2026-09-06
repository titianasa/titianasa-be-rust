use crate::services::speaking_room::{HistoryEntry, LanguageInput, ScenarioInput, TutorPersonaInput};

#[derive(Debug, serde::Deserialize)]
pub struct PostTurnRequest {
    pub scenario: Option<ScenarioInput>,
    pub level: Option<String>,
    pub user_message: String,
    pub history: Option<Vec<HistoryEntry>>,
    pub tutor_persona: Option<TutorPersonaInput>,
    pub mode: Option<String>,
    pub language: Option<LanguageInput>,
}

#[derive(Debug, serde::Deserialize)]
pub struct PostSummaryRequest {
    pub messages: Option<Vec<HistoryEntry>>,
    pub scenario: Option<ScenarioInput>,
    pub level: Option<String>,
    pub language: Option<LanguageInput>,
}

#[derive(Debug, serde::Deserialize)]
pub struct PostTtsRequest {
    pub text: String,
    pub voice: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct PostTranscribeRequest {
    pub audio_base64: String,
    pub mime_type: Option<String>,
}
