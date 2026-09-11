use sqlx::postgres::{PgPool, PgPoolOptions};

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    Ok(pool)
}

fn already_applied(err: &sqlx::Error) -> bool {
    let msg = err.to_string();
    msg.contains("already exists") || msg.contains("duplicate")
}

// This project's migrations/ run against the SAME local dev Postgres the
// retired Bun backend used, and some migrations were applied by hand
// there — their objects exist but `_sqlx_migrations` has no row for them.
//
// sqlx's own `Migrator::run` stops at the first such migration, and the
// old wrapper downgraded that to a warning: every migration AFTER it was
// then silently never applied (0040 blocked 0041-0044 this way). So each
// migration runs on its own instead: applied ones are skipped; one that
// fails with "already exists" is recorded as applied and the run moves
// on; anything else still aborts startup loudly.
pub async fn run_migrations(pool: &PgPool) -> anyhow::Result<()> {
    use sqlx::Executor;

    let migrator = sqlx::migrate!("./migrations");
    pool.execute(
        "create table if not exists _sqlx_migrations (
            version bigint primary key,
            description text not null,
            installed_on timestamptz not null default now(),
            success boolean not null,
            checksum bytea not null,
            execution_time bigint not null
        )",
    )
    .await?;
    let applied: std::collections::HashSet<i64> = sqlx::query_scalar::<_, i64>("select version from _sqlx_migrations where success")
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();

    for migration in migrator.iter() {
        if applied.contains(&migration.version) || migration.migration_type.is_down_migration() {
            continue;
        }
        let started = std::time::Instant::now();
        let mut tx = pool.begin().await?;
        let outcome = (&mut *tx).execute(migration.sql.as_ref()).await;
        match outcome {
            Ok(_) => {
                tx.commit().await?;
                tracing::info!(version = migration.version, description = %migration.description, "migration applied");
            }
            Err(e) if already_applied(&e) => {
                tx.rollback().await?;
                tracing::warn!(version = migration.version, description = %migration.description, error = %e, "migration's objects already exist — recording it as applied");
            }
            Err(e) => return Err(anyhow::anyhow!("migration {} ({}) failed: {e}", migration.version, migration.description)),
        }
        sqlx::query("insert into _sqlx_migrations (version, description, success, checksum, execution_time) values ($1, $2, true, $3, $4) on conflict (version) do nothing")
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .bind(started.elapsed().as_nanos() as i64)
            .execute(pool)
            .await?;
    }
    Ok(())
}
