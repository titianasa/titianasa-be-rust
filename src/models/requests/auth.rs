#[derive(Debug, serde::Deserialize)]
pub struct GoogleCallbackRequest {
    pub id_token: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}
