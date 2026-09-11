use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use uuid::Uuid;

use crate::errors::AppError;

// Phase 35 (M3) — a student's durable, per-account QR "attendance card"
// payload: shown on their own Profil page, scanned by a teacher in
// "Scan siswa" mode (handlers/org_attendance.rs). Signed with the SAME
// secret every other token in this app already uses
// (config.jwt_access_secret, reused rather than provisioning a second
// secret for one narrow use) but domain-separated via `purpose`, so a
// real login access token can never be replayed here and vice versa.
// A 5-year expiry, not none — jsonwebtoken's default Validation
// requires `exp` to be present, and this is meant to behave like a
// physical ID card (same code works every time), not a short-lived
// session token.
const PURPOSE: &str = "attendance-qr";
const TTL_SECONDS: i64 = 5 * 365 * 24 * 60 * 60;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct AttendanceQrClaims {
    sub: String,
    purpose: String,
    iat: i64,
    exp: i64,
}

pub fn issue_student_token(secret: &str, user_id: Uuid) -> Result<String, AppError> {
    let now = Utc::now().timestamp();
    let claims = AttendanceQrClaims { sub: user_id.to_string(), purpose: PURPOSE.to_string(), iat: now, exp: now + TTL_SECONDS };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))
}

pub fn verify_student_token(token: &str, secret: &str) -> Result<Uuid, AppError> {
    let validation = Validation::new(Algorithm::HS256);
    let data = decode::<AttendanceQrClaims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map_err(|_| AppError::UnprocessableEntity("invalid_qr_token", "kode QR tidak valid atau sudah kedaluwarsa".to_string()))?;
    if data.claims.purpose != PURPOSE {
        return Err(AppError::UnprocessableEntity("invalid_qr_token", "kode QR tidak valid".to_string()));
    }
    Uuid::parse_str(&data.claims.sub).map_err(|_| AppError::UnprocessableEntity("invalid_qr_token", "kode QR tidak valid".to_string()))
}
