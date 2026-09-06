use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;

// `payout_earned`/`payout_withdrawn` are RAW RUPIAH (not credit) — a
// different unit from `earn`/`spend`/`purchase`/`refund` (the AI/
// Diamond credit economy, ADR-0005). Marketplace booking money and AI
// credit are deliberately NOT interchangeable and never converted
// between each other — this module never touches the `credits` table
// or `services::economy`'s `charge`/`ensure_credits_row` at all.
pub async fn record_payout(pool: &PgPool, tutor_id: Uuid, amount_idr: i64, reference: &str) -> Result<(), AppError> {
    sqlx::query!(r#"insert into transactions (user_id, type, amount, reference) values ($1, 'payout_earned', $2, $3)"#, tutor_id, amount_idr, reference)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn record_refund(pool: &PgPool, student_id: Uuid, amount_idr: i64, reference: &str) -> Result<(), AppError> {
    sqlx::query!(r#"insert into transactions (user_id, type, amount, reference) values ($1, 'refund', $2, $3)"#, student_id, amount_idr, reference)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct WalletResponse {
    pub balance_idr: i64,
}

// GET /tutors/me/wallet — deliberately ignores type='refund' rows (only
// sums payout_earned - payout_withdrawn); nothing ever writes
// payout_withdrawn (no withdrawal endpoint exists yet).
pub async fn get_wallet(pool: &PgPool, ctx: &AuthContext) -> Result<WalletResponse, AppError> {
    let row = sqlx::query!(
        r#"select
             coalesce(sum(case when type = 'payout_earned' then amount else 0 end), 0)::bigint as "earned!",
             coalesce(sum(case when type = 'payout_withdrawn' then amount else 0 end), 0)::bigint as "withdrawn!"
           from transactions where user_id = $1"#,
        ctx.user_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(WalletResponse { balance_idr: row.earned - row.withdrawn })
}
