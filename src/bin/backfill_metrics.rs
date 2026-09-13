// P40-004 (ADR-0014 §3) — one-time backfill of `metrics_daily_*` from
// the earliest source data through today. Safe to run more than once
// (every rollup query is `on conflict do update`, same idempotency the
// recurring `metrics_rollup` job relies on).
//
// Usage: `cargo run --bin backfill_metrics` (reads DATABASE_URL the
// same way the server does, via `.env`/the environment).

use titian_backend_rust::services::metrics_rollup;
use titian_backend_rust::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = titian_backend_rust::db::connect(&config.database_url).await?;

    let report = metrics_rollup::backfill(&pool).await?;

    match report.earliest_day {
        Some(day) => println!("=== backfill agregat harian selesai ===\nhari pertama data: {day}\nhari diproses    : {}", report.days_processed),
        None => println!("=== tidak ada data sumber (orders/users/ai_tasks kosong) — tidak ada yang di-backfill ==="),
    }

    Ok(())
}
