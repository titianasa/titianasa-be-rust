use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;

// Idempotent — inserts a zero-balance credits row if this user doesn't
// have one yet.
pub async fn ensure_credits_row(pool: &PgPool, user_id: Uuid) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into credits (user_id, balance) values ($1, 0)
           on conflict (user_id) do update set user_id = credits.user_id"#,
        user_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// P11-003 — no scheduling infra exists in this backend, so this runs
// lazily instead, called from get_balance/charge. Each expired grant
// gets a paired `expire` transaction (audit trail, never an UPDATE/
// DELETE of the original row) — idempotent against a 2nd sequential
// call.
pub async fn sweep_expired_allowance(pool: &PgPool, user_id: Uuid) -> Result<(), AppError> {
    struct ExpiredRow {
        id: Uuid,
        amount: i64,
    }
    let expired = sqlx::query_as!(
        ExpiredRow,
        r#"select id, amount from transactions
           where user_id = $1 and type = 'earn' and reference like 'subscription:%'
             and expires_at is not null and expires_at <= now()"#,
        user_id,
    )
    .fetch_all(pool)
    .await?;

    for original in expired {
        let reversal_reference = format!("expire:{}", original.id);
        let already_reversed = sqlx::query_scalar!(
            r#"select id from transactions where user_id = $1 and type = 'expire' and reference = $2"#,
            user_id,
            reversal_reference,
        )
        .fetch_optional(pool)
        .await?;
        if already_reversed.is_some() {
            continue;
        }

        let mut tx = pool.begin().await?;
        sqlx::query!(
            r#"insert into transactions (user_id, type, amount, reference) values ($1, 'expire', $2, $3)"#,
            user_id,
            -original.amount,
            reversal_reference,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(r#"update credits set balance = balance - $2, updated_at = now() where user_id = $1"#, user_id, original.amount)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

// Sweeps expired subscription allowance BEFORE reading, so the balance
// returned always reflects reality even though no background job ever
// runs.
pub async fn get_balance(pool: &PgPool, user_id: Uuid) -> Result<i64, AppError> {
    sweep_expired_allowance(pool, user_id).await?;
    let balance = sqlx::query_scalar!(r#"select balance from credits where user_id = $1"#, user_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("no credits row for user {user_id}")))?;
    Ok(balance)
}

// Writes one transactions row (type='spend', negative amount) and
// decrements the cached credits.balance in the same DB transaction.
pub async fn charge(pool: &PgPool, user_id: Uuid, amount: i64, reference: &str) -> Result<(), AppError> {
    sweep_expired_allowance(pool, user_id).await?;
    let mut tx = pool.begin().await?;
    sqlx::query!(r#"insert into transactions (user_id, type, amount, reference) values ($1, 'spend', $2, $3)"#, user_id, -amount, reference)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(r#"update credits set balance = balance - $2, updated_at = now() where user_id = $1"#, user_id, amount).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

// Grants subscription allowance. `expires_at` set (grace-period end)
// distinguishes this from a direct purchase, which never expires.
pub async fn grant_allowance(pool: &PgPool, user_id: Uuid, amount: i64, reference: &str, expires_at: DateTime<Utc>) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        r#"insert into transactions (user_id, type, amount, reference, expires_at) values ($1, 'earn', $2, $3, $4)"#,
        user_id,
        amount,
        reference,
        expires_at,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(r#"update credits set balance = balance + $2, updated_at = now() where user_id = $1"#, user_id, amount).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

// GET /me/credits — always "my own" balance. ensure_credits_row first
// so a user who has never triggered any credit activity gets 0, not an
// error.
pub async fn get_my_balance(pool: &PgPool, ctx: &AuthContext) -> Result<i64, AppError> {
    ensure_credits_row(pool, ctx.user_id).await?;
    get_balance(pool, ctx.user_id).await
}
