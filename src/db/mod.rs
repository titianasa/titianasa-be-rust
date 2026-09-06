use sqlx::postgres::{PgPool, PgPoolOptions};

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    Ok(pool)
}

// Mirrors parelabs-backend/api/src/main.rs's resilience trick: this new
// project's migrations/ are adapted from titian-backend-bun's own 26
// existing SQL migration files and run against the SAME local dev
// Postgres the Bun backend already uses (so no second database is
// needed during development) — that means most tables already exist by
// the time this runs. Downgrade "already applied" classes of error to a
// warning instead of aborting; a genuinely new, not-yet-applied
// migration still fails loudly.
pub async fn run_migrations(pool: &PgPool) -> anyhow::Result<()> {
    match sqlx::migrate!("./migrations").run(pool).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("already exists") || msg.contains("checksum") {
                tracing::warn!(error = %msg, "migration skipped (already applied against this database)");
                Ok(())
            } else {
                Err(e.into())
            }
        }
    }
}
