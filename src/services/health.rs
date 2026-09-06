use sqlx::PgPool;

// Port of health_service.ts's check. Redis was deliberately left
// "not_configured" through the whole Bun era (see phase-1.md's P1-012 —
// nothing needed it yet) — now genuinely pinged, closing that deferral.
#[derive(serde::Serialize)]
pub struct HealthStatus {
    pub status: &'static str,
    pub db: &'static str,
    pub redis: &'static str,
}

async fn check_redis(client: &redis::Client) -> bool {
    let Ok(mut conn) = client.get_multiplexed_tokio_connection().await else { return false };
    redis::cmd("PING").query_async::<_, String>(&mut conn).await.is_ok()
}

pub async fn check(pool: &PgPool, redis_client: &redis::Client) -> HealthStatus {
    let db_ok = sqlx::query_scalar!("select 1").fetch_one(pool).await.is_ok();
    let redis_ok = check_redis(redis_client).await;
    HealthStatus {
        status: if db_ok && redis_ok { "ok" } else { "degraded" },
        db: if db_ok { "ok" } else { "down" },
        redis: if redis_ok { "ok" } else { "down" },
    }
}
