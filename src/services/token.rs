use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::errors::AppError;
use crate::models::auth::Claims;

// Port of token_service.ts's issueAccessToken.
pub fn issue_access_token(
    secret: &str,
    user_id: &str,
    email: &str,
    ttl_minutes: i64,
) -> anyhow::Result<String> {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: user_id.to_string(),
        email: email.to_string(),
        iat: now,
        exp: now + ttl_minutes * 60,
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

// Port of token_service.ts's verifyAccessToken — used by
// middleware/auth.rs, the guard every protected route relies on.
pub fn verify_access_token(token: &str, secret: &str) -> Result<Claims, AppError> {
    let validation = Validation::new(Algorithm::HS256);
    let data = decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map_err(|_| AppError::Unauthorized)?;
    Ok(data.claims)
}

// Port of token_service.ts's generateRefreshToken — opaque, not a JWT;
// looked up (and revoked) by database row hash, never decoded.
// Returns (raw token for the client, sha256 hex to store).
pub fn generate_refresh_token() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = URL_SAFE_NO_PAD.encode(bytes);
    let hash = hash_refresh_token(&raw);
    (raw, hash)
}

// Port of token_service.ts's hashRefreshToken (was `Bun.CryptoHasher`).
pub fn hash_refresh_token(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
