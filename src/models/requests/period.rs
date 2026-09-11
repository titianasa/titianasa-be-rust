#[derive(Debug, serde::Deserialize)]
pub struct CreatePeriodRequest {
    pub name: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}
