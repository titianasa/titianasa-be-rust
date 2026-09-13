#[derive(Debug, serde::Deserialize)]
pub struct AuditLogQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

// P40-003 — request bodies for Pengaturan AI reuse ai_settings' own
// input structs directly (SaveRoleInput/PatchCatalogInput), same as
// every other admin write in this file: one shape, not a duplicate DTO
// that could quietly drift from what the service actually accepts.

// P40-005 — `from`/`to` for the metrics pages that take a period
// (Penjualan, Operasional AI). Missing entirely = the last 7 WIB days,
// applied in the handler (`admin_metrics::default_period`) since that's
// where "today" is computed — this struct stays a dumb wire shape.
#[derive(Debug, serde::Deserialize)]
pub struct MetricsPeriodQuery {
    pub from: Option<chrono::NaiveDate>,
    pub to: Option<chrono::NaiveDate>,
}
