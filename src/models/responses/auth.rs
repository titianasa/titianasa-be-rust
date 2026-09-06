use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct UserSummary {
    pub id: Uuid,
    pub email: String,
    pub name: String,
}

// Port of auth_handler.ts's LoginResponse.
#[derive(Debug, serde::Serialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub user: UserSummary,
}

// Port of auth_handler.ts's RefreshResponse.
#[derive(Debug, serde::Serialize)]
pub struct RefreshResponse {
    pub access_token: String,
}

#[derive(Debug, serde::Serialize)]
pub struct RoleEntry {
    pub organization_id: Uuid,
    pub role: String,
}

// Port of user_handler.ts's MeResponse.
#[derive(Debug, serde::Serialize)]
pub struct MeResponse {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub roles: Vec<RoleEntry>,
}
