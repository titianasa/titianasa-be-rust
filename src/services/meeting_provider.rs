use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use rand::RngCore;

use crate::errors::AppError;

// Port of meeting_provider.ts. Provider abstraction (ADR-0004's
// precedent, same shape as AIProvider/PaymentProvider) — business logic
// never talks to a video-conferencing API directly, always through this
// trait, injected via AppState.

#[derive(Debug, Clone)]
pub struct CreateMeetingResult {
    pub external_meeting_id: String,
    pub join_url: String,
}

// 1 row per person who appeared in the meeting, already aggregated
// across however many times they left/rejoined.
#[derive(Debug, Clone)]
pub struct ParticipantReportEntry {
    pub external_participant_name: Option<String>,
    pub external_google_account_id: Option<String>,
    pub first_joined_at: DateTime<Utc>,
    pub last_left_at: DateTime<Utc>,
    pub duration_seconds: i32,
    pub join_session_count: i32,
}

// State mirrors the Meet API's own recording lifecycle values rather
// than collapsing into a boolean — a caller showing "still processing"
// vs "no recording yet" needs to tell those apart.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    NotFound,
    InProgress,
    Ready,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordingStatus {
    pub state: RecordingState,
    pub drive_url: Option<String>,
}

#[async_trait::async_trait]
pub trait MeetingProvider: Send + Sync {
    // Stored verbatim into class_sessions.meeting_provider — the source
    // of truth for which provider actually created a given session.
    fn name(&self) -> &'static str;
    async fn create_meeting(&self, title: &str, scheduled_start: DateTime<Utc>, scheduled_end: DateTime<Utc>) -> Result<CreateMeetingResult, AppError>;
    async fn get_participant_report(&self, external_meeting_id: &str) -> Result<Vec<ParticipantReportEntry>, AppError>;
    // Recordings live in the ORGANIZER's own Google Drive, never
    // fetched/stored by this app — this just resolves a Drive link for
    // the UI to point at.
    async fn get_recording_status(&self, external_meeting_id: &str) -> Result<RecordingStatus, AppError>;
}

// Generates a fake meeting id + join URL, both clearly recognizable as
// stub output — no network call ever made. The ONLY implementation used
// when GOOGLE_MEET_* credentials aren't configured (local dev / CI).
pub struct StubMeetingProvider;

#[async_trait::async_trait]
impl MeetingProvider for StubMeetingProvider {
    fn name(&self) -> &'static str {
        "stub"
    }

    async fn create_meeting(&self, _title: &str, _scheduled_start: DateTime<Utc>, _scheduled_end: DateTime<Utc>) -> Result<CreateMeetingResult, AppError> {
        let mut bytes = [0u8; 9];
        rand::thread_rng().fill_bytes(&mut bytes);
        let external_meeting_id = format!("stub_{}", URL_SAFE_NO_PAD.encode(bytes));
        let join_url = format!("https://stub-meet.titianasa.dev/{external_meeting_id}");
        Ok(CreateMeetingResult { external_meeting_id, join_url })
    }

    // Never called in practice for the stub path (syncAttendance skips
    // it) — kept so the trait is genuinely implementable, not
    // partially-stubbed.
    async fn get_participant_report(&self, _external_meeting_id: &str) -> Result<Vec<ParticipantReportEntry>, AppError> {
        Ok(vec![])
    }

    async fn get_recording_status(&self, _external_meeting_id: &str) -> Result<RecordingStatus, AppError> {
        Ok(RecordingStatus { state: RecordingState::NotFound, drive_url: None })
    }
}

// Real Google Meet REST API implementation. Single centralized
// "organizer" account model — one refresh token for one account creates
// every class session's meeting, not a per-tutor connected account.
// Auth: a long-lived refresh token is exchanged for a short-lived access
// token on EVERY call — no in-process caching, matching the Bun
// implementation exactly (simple enough not to need a token-caching
// layer for this call volume).
pub struct GoogleMeetProvider {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    client: reqwest::Client,
}

impl GoogleMeetProvider {
    pub fn new(client_id: String, client_secret: String, refresh_token: String) -> Self {
        Self { client_id, client_secret, refresh_token, client: reqwest::Client::new() }
    }

    async fn get_access_token(&self) -> Result<String, AppError> {
        let res = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .form(&[
                ("refresh_token", self.refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| AppError::BadGateway("meeting_provider_auth_failed", e.to_string()))?;

        let ok = res.status().is_success();
        let json: serde_json::Value = res.json().await.unwrap_or(serde_json::Value::Null);
        if !ok {
            return Err(AppError::BadGateway("meeting_provider_auth_failed", json.to_string()));
        }
        json.get("access_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| AppError::BadGateway("meeting_provider_auth_failed", json.to_string()))
    }

    async fn call_get(&self, path: &str) -> Result<serde_json::Value, AppError> {
        let access_token = self.get_access_token().await?;
        let res = self
            .client
            .get(format!("https://meet.googleapis.com/v2/{path}"))
            .header("authorization", format!("Bearer {access_token}"))
            .send()
            .await
            .map_err(|e| AppError::BadGateway("meeting_provider_request_failed", e.to_string()))?;
        self.parse_response(res).await
    }

    async fn call_post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, AppError> {
        let access_token = self.get_access_token().await?;
        let res = self
            .client
            .post(format!("https://meet.googleapis.com/v2/{path}"))
            .header("authorization", format!("Bearer {access_token}"))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::BadGateway("meeting_provider_request_failed", e.to_string()))?;
        self.parse_response(res).await
    }

    async fn parse_response(&self, res: reqwest::Response) -> Result<serde_json::Value, AppError> {
        let ok = res.status().is_success();
        let json: serde_json::Value = res.json().await.unwrap_or(serde_json::Value::Null);
        if !ok {
            return Err(AppError::BadGateway("meeting_provider_request_failed", json.to_string()));
        }
        Ok(json)
    }

    // externalMeetingId is the space resource name (e.g. "spaces/abc123",
    // exactly what create_meeting returned) — conferenceRecords are
    // looked up BY space; a space can in principle have accumulated
    // multiple conference occurrences (e.g. a recurring class reusing
    // the same link), so this takes the most recent one. Shared by
    // get_participant_report and get_recording_status.
    async fn find_most_recent_conference_record(&self, external_meeting_id: &str) -> Result<Option<String>, AppError> {
        let filter = format!("space.name = \"{external_meeting_id}\"");
        let encoded_filter = urlencoding_encode(&filter);
        let list_json = self.call_get(&format!("conferenceRecords?filter={encoded_filter}")).await?;
        let records = list_json.get("conferenceRecords").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if records.is_empty() {
            return Ok(None);
        }
        let most_recent = records
            .into_iter()
            .max_by(|a, b| {
                let a_start = a.get("startTime").and_then(|v| v.as_str()).unwrap_or("");
                let b_start = b.get("startTime").and_then(|v| v.as_str()).unwrap_or("");
                a_start.cmp(b_start)
            })
            .unwrap();
        Ok(most_recent.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
    }
}

#[async_trait::async_trait]
impl MeetingProvider for GoogleMeetProvider {
    fn name(&self) -> &'static str {
        "google_meet"
    }

    // scheduled_start/scheduled_end aren't sent to Meet — a Meet "space"
    // has no built-in scheduled-time concept of its own; the schedule
    // Titian Asa cares about lives entirely in class_sessions.
    async fn create_meeting(&self, _title: &str, _scheduled_start: DateTime<Utc>, _scheduled_end: DateTime<Utc>) -> Result<CreateMeetingResult, AppError> {
        let json = self
            .call_post(
                "spaces",
                serde_json::json!({
                    "config": {"accessType": "OPEN", "artifactConfig": {"recordingConfig": {"autoRecordingGeneration": "ON"}}},
                }),
            )
            .await?;
        let external_meeting_id = json.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let join_url = json.get("meetingUri").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        Ok(CreateMeetingResult { external_meeting_id, join_url })
    }

    async fn get_participant_report(&self, external_meeting_id: &str) -> Result<Vec<ParticipantReportEntry>, AppError> {
        let Some(conference_record) = self.find_most_recent_conference_record(external_meeting_id).await? else {
            return Ok(vec![]);
        };
        let participants_json = self.call_get(&format!("{conference_record}/participants")).await?;
        let participants = participants_json.get("participants").and_then(|v| v.as_array()).cloned().unwrap_or_default();

        Ok(participants
            .into_iter()
            .filter_map(|p| {
                // signedinUser.user is "users/{numeric_google_account_id}"
                // — a phone-in/anonymous participant has no signedinUser
                // at all and is treated as unmatched (null id), never
                // guessed from displayName.
                let google_account_id =
                    p.get("signedinUser").and_then(|s| s.get("user")).and_then(|v| v.as_str()).map(|s| s.trim_start_matches("users/").to_string());
                let name = p
                    .get("signedinUser")
                    .and_then(|s| s.get("displayName"))
                    .or_else(|| p.get("anonymousUser").and_then(|a| a.get("displayName")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let first_joined_at = p.get("earliestStartTime").and_then(|v| v.as_str()).and_then(|s| s.parse::<DateTime<Utc>>().ok())?;
                let last_left_at = p.get("latestEndTime").and_then(|v| v.as_str()).and_then(|s| s.parse::<DateTime<Utc>>().ok())?;
                let duration_seconds = (last_left_at - first_joined_at).num_seconds().max(0) as i32;
                Some(ParticipantReportEntry {
                    external_participant_name: name,
                    external_google_account_id: google_account_id,
                    first_joined_at,
                    last_left_at,
                    duration_seconds,
                    // MVP: not fetching the participantSessions
                    // sub-resource for an exact rejoin count —
                    // earliest/latest already covers the duration math
                    // that matters for attendance verification.
                    join_session_count: 1,
                })
            })
            .collect())
    }

    async fn get_recording_status(&self, external_meeting_id: &str) -> Result<RecordingStatus, AppError> {
        let Some(conference_record) = self.find_most_recent_conference_record(external_meeting_id).await? else {
            return Ok(RecordingStatus { state: RecordingState::NotFound, drive_url: None });
        };
        let recordings_json = self.call_get(&format!("{conference_record}/recordings")).await?;
        let recordings = recordings_json.get("recordings").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let Some(recording) = recordings.first() else {
            return Ok(RecordingStatus { state: RecordingState::NotFound, drive_url: None });
        };

        if recording.get("state").and_then(|v| v.as_str()) != Some("FILE_GENERATED") {
            return Ok(RecordingStatus { state: RecordingState::InProgress, drive_url: None });
        }
        let drive_url = recording.get("driveDestination").and_then(|d| d.get("exportUri")).and_then(|v| v.as_str()).map(|s| s.to_string());
        Ok(RecordingStatus { state: RecordingState::Ready, drive_url })
    }
}

// Minimal RFC 3986 percent-encoding for a single query-string value
// (matches JS's `encodeURIComponent`) — no extra crate needed for one
// call site.
fn urlencoding_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
