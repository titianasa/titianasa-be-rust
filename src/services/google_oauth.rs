use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::errors::AppError;
use crate::models::auth::GoogleClaims;

const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
// Google rotates signing keys infrequently; re-fetching on every login
// would be wasteful and add real latency to the hot path. Mirrors the
// resolveMaxTokens-style module-level cache already established in the
// Bun codebase (ai_provider.ts) for a different endpoint.
const JWKS_CACHE_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, serde::Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Debug, serde::Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

// Port of google_oauth.ts's GoogleTokenVerifier — verifies Google
// id_tokens server-side against Google's real JWKS. No single crate
// does the fetch-JWKS + verify-by-kid dance end to end the way jose's
// createRemoteJWKSet does, so this is hand-rolled: cache the key set,
// pick the right key by the token's `kid` header, then verify RS256 +
// issuer + audience + expiry.
enum Source {
    Live { client: reqwest::Client, cache: RwLock<Option<(Instant, Vec<Jwk>)>> },
    // Port of google_oauth.ts's GoogleTokenVerifier.withSeededJwk — lets
    // tests exercise the exact same `.verify()` path a real HTTP request
    // goes through (kid lookup, signature + claims validation) with a
    // locally generated keypair, never touching Google's live JWKS.
    Seeded { kid: String, key: DecodingKey },
}

pub struct GoogleTokenVerifier {
    source: Source,
}

impl GoogleTokenVerifier {
    pub fn new() -> Self {
        Self {
            source: Source::Live { client: reqwest::Client::new(), cache: RwLock::new(None) },
        }
    }

    pub fn with_seeded_key(kid: impl Into<String>, key: DecodingKey) -> Self {
        Self { source: Source::Seeded { kid: kid.into(), key } }
    }

    async fn fetch_keys(client: &reqwest::Client, cache: &RwLock<Option<(Instant, Vec<Jwk>)>>) -> anyhow::Result<Vec<Jwk>> {
        {
            let guard = cache.read().await;
            if let Some((fetched_at, keys)) = guard.as_ref() {
                if fetched_at.elapsed() < JWKS_CACHE_TTL {
                    return Ok(keys.clone());
                }
            }
        }
        let resp: JwksResponse = client.get(GOOGLE_JWKS_URL).send().await?.json().await?;
        let mut guard = cache.write().await;
        *guard = Some((Instant::now(), resp.keys.clone()));
        Ok(resp.keys)
    }

    pub async fn verify(&self, id_token: &str, client_id: &str) -> Result<GoogleClaims, AppError> {
        let header = decode_header(id_token).map_err(|_| AppError::InvalidToken)?;
        let kid = header.kid.ok_or(AppError::InvalidToken)?;

        match &self.source {
            Source::Live { client, cache } => {
                let keys = Self::fetch_keys(client, cache).await.map_err(|_| AppError::InvalidToken)?;
                let jwk = keys.iter().find(|k| k.kid == kid).ok_or(AppError::InvalidToken)?;
                let key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|_| AppError::InvalidToken)?;
                verify_claims_with_key(id_token, &key, client_id)
            }
            Source::Seeded { kid: seeded_kid, key } => {
                if &kid != seeded_kid {
                    return Err(AppError::InvalidToken);
                }
                verify_claims_with_key(id_token, key, client_id)
            }
        }
    }
}

impl Default for GoogleTokenVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// The actual claim-validation logic (signature, aud, iss, exp), factored
// out from key resolution — lets tests exercise the real verification
// path with a locally generated keypair instead of hitting Google's live
// JWKS endpoint. Direct port of google_oauth.ts's verifyClaimsWithKey.
pub fn verify_claims_with_key(
    id_token: &str,
    key: &DecodingKey,
    client_id: &str,
) -> Result<GoogleClaims, AppError> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&GOOGLE_ISSUERS);
    let data = decode::<GoogleClaims>(id_token, key, &validation).map_err(|_| AppError::InvalidToken)?;
    Ok(data.claims)
}
