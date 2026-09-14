// Phase 1 of Pabrik Konten (ADR-0015) — one-time seed of
// `curriculum_standards` and `generation_exemplars` from the 394 topics
// whose `metadata.bab_plan` was authored by Claude (Fisika + Kimia).
// Safe to run more than once: standards are inserted only where a tahap
// has none yet, exemplars are upserted per domain.
//
// Usage: `cargo run --bin seed_content_factory` (reads DATABASE_URL the
// same way the server does, via `.env`/the environment).

use uuid::Uuid;

use titian_backend_rust::models::auth::AuthContext;
use titian_backend_rust::services::content_factory::admin;
use titian_backend_rust::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = titian_backend_rust::db::connect(&config.database_url).await?;

    // ndsanja@gmail.com — the platform_admin account, so seeded standards
    // and exemplars are attributed to a real admin rather than a null user.
    let admin_user_id: Uuid = "e0b7f9ab-228d-449d-8fd5-9b7cd4d51a77".parse()?;
    let system = AuthContext { user_id: admin_user_id, organization_id: None, role: Some("platform_admin".to_string()) };
    let report = admin::seed_from_metadata(&pool, &system).await?;

    println!("=== seed Pabrik Konten selesai ===\nstandar kurikulum baru : {}\ncontoh emas diperbarui : {}", report.standards_created, report.exemplars_upserted);

    Ok(())
}
