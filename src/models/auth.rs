use uuid::Uuid;

// Port of token_service.ts's AccessTokenClaims.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Claims {
    pub sub: String,
    pub email: String,
    pub iat: i64,
    pub exp: i64,
}

// Port of auth-context.ts's AuthContext — the exact shape every
// protected handler receives, resolved by middleware/auth.rs. Not
// parelabs-backend's `AuthUser` shape: titian's has an org-scoped
// role, not a single global_role.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: Uuid,
    pub organization_id: Option<Uuid>,
    pub role: Option<String>,
}

// Port of google_oauth.ts's GoogleClaims — the claims lifted out of a
// verified Google id_token.
#[derive(Debug, serde::Deserialize)]
pub struct GoogleClaims {
    pub sub: String,
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    pub name: Option<String>,
    pub picture: Option<String>,
}
