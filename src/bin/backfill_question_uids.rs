// P39-001 (ADR-0013 L1) — one-time backfill: every question written
// before `QuizQuestion.uid` existed gets a permanent id now, using the
// exact same `ensure_question_uids` logic every live save path already
// goes through (`module_item::update_quiz_config`,
// `quiz_generation::generate_quiz_group`) — so a question backfilled
// here and one saved through the API a moment later are indistinguishable.
//
// Not a SQL migration: `uid` lives inside the `quiz_config` jsonb blob,
// not a new column, and correctly walking every subtype's nested shapes
// (flow steps, table rows, passage sections — none of which need a uid
// themselves, only `questions[]` does) is exactly the logic
// `quiz_config_schema::parse` + `ensure_question_uids` already embody.
// Reimplementing that in raw jsonb SQL would be a second, easily
// divergent copy of the same rule.
//
// Idempotent and safe to run more than once: a row whose questions
// already all have unique uids is left untouched (`ensure_question_uids`
// reports `changed = false` and this skips the write).
//
// Usage: `cargo run --bin backfill_question_uids` (reads DATABASE_URL
// the same way the server does, via `.env`/the environment).

use titian_backend_rust::services::{quiz_config, quiz_config_schema};
use titian_backend_rust::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = titian_backend_rust::db::connect(&config.database_url).await?;

    let rows = sqlx::query!(
        r#"select id, quiz_config as "quiz_config!" from module_items where content_type = 'quiz' and quiz_config is not null"#
    )
    .fetch_all(&pool)
    .await?;

    let total = rows.len();
    let mut updated = 0usize;
    let mut already_clean = 0usize;
    let mut unparseable = Vec::new();

    for row in rows {
        let mut parsed = match quiz_config_schema::parse(&row.quiz_config) {
            Ok(p) => p,
            Err(e) => {
                unparseable.push((row.id, format!("{e:?}")));
                continue;
            }
        };

        if !quiz_config::ensure_question_uids(&mut parsed) {
            already_clean += 1;
            continue;
        }

        let value = serde_json::to_value(&parsed)?;
        sqlx::query!("update module_items set quiz_config = $2 where id = $1", row.id, value).execute(&pool).await?;
        updated += 1;
    }

    println!("=== backfill uid soal selesai ===");
    println!("total item quiz diperiksa : {total}");
    println!("diperbarui (uid ditambah) : {updated}");
    println!("sudah bersih (dilewati)   : {already_clean}");
    println!("tidak bisa di-parse       : {}", unparseable.len());
    for (id, err) in &unparseable {
        eprintln!("  - {id}: {err}");
    }
    if !unparseable.is_empty() {
        eprintln!(
            "\nPERINGATAN: {} item quiz_config tidak valid dan DILEWATI (bukan error backend — kemungkinan data lama yang sudah rusak sebelum P39-001). Periksa manual.",
            unparseable.len()
        );
    }

    Ok(())
}
