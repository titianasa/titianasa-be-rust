use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::cohort::{self, can_manage_cohorts};
use crate::services::learning_product;

#[derive(Debug, serde::Serialize, Clone)]
pub struct EnrollmentResponse {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub student_id: Uuid,
    pub status: String,
    pub enrolled_at: DateTime<Utc>,
    pub sessions_remaining: Option<i32>,
}

#[derive(Debug, serde::Serialize)]
pub struct StudentRowResponse {
    #[serde(flatten)]
    pub enrollment: EnrollmentResponse,
    pub student_name: String,
    pub student_email: String,
}

#[derive(Debug, serde::Serialize)]
pub struct StudentListResponse {
    pub items: Vec<StudentRowResponse>,
}

#[derive(Debug, serde::Serialize)]
pub struct MyEnrollmentResponse {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub cohort_name: String,
    pub cohort_starts_at: Option<DateTime<Utc>>,
    pub product_id: Uuid,
    pub product_title: String,
    pub product_type: String,
    pub product_delivery_mode: String,
    pub tutor_id: Uuid,
    pub status: String,
    pub order_status: Option<String>,
    pub enrolled_at: DateTime<Utc>,
    pub sessions_remaining: Option<i32>,
}

#[derive(Debug, serde::Serialize)]
pub struct MyEnrollmentListResponse {
    pub items: Vec<MyEnrollmentResponse>,
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<EnrollmentResponse>, AppError> {
    let row = sqlx::query_as!(
        EnrollmentResponse,
        r#"select id, cohort_id, student_id, status, enrolled_at, sessions_remaining from enrollments where id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn find_by_cohort_and_student(pool: &PgPool, cohort_id: Uuid, student_id: Uuid) -> Result<Option<EnrollmentResponse>, AppError> {
    let row = sqlx::query_as!(
        EnrollmentResponse,
        r#"select id, cohort_id, student_id, status, enrolled_at, sessions_remaining from enrollments
           where cohort_id = $1 and student_id = $2"#,
        cohort_id,
        student_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn count_active_by_cohort(pool: &PgPool, cohort_id: Uuid) -> Result<i64, AppError> {
    let count = sqlx::query_scalar!(
        r#"select count(*) as "count!" from enrollments where cohort_id = $1 and status in ('pending', 'active')"#,
        cohort_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(count)
}

// Exposed for order.rs's webhook handler (pending -> active on payment).
pub async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> Result<EnrollmentResponse, AppError> {
    let row = sqlx::query_as!(
        EnrollmentResponse,
        r#"update enrollments set status = $2 where id = $1
           returning id, cohort_id, student_id, status, enrolled_at, sessions_remaining"#,
        id,
        status,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn cancel(pool: &PgPool, id: Uuid, cancelled_by: &str) -> Result<EnrollmentResponse, AppError> {
    let row = sqlx::query_as!(
        EnrollmentResponse,
        r#"update enrollments set status = 'cancelled', cancelled_by = $2 where id = $1
           returning id, cohort_id, student_id, status, enrolled_at, sessions_remaining"#,
        id,
        cancelled_by,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// POST /cohorts/{id}/enrollments — self-enroll only, student_id always
// ctx.user_id. A repeat call is idempotent (returns the existing row,
// checked BEFORE the capacity gate so an already-enrolled student is
// never rejected by a since-filled cohort).
//
// Race-condition handling — explicitly none, matching Bun: no
// SELECT...FOR UPDATE, no advisory lock, no serializable transaction.
// Documented accepted MVP limitation, reproduced as-is.
pub async fn enroll_self(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<EnrollmentResponse, AppError> {
    let cohort_row = cohort::find_by_id(pool, cohort_id).await?.ok_or(AppError::NotFound("cohort_not_found"))?;

    if let Some(existing) = find_by_cohort_and_student(pool, cohort_id, ctx.user_id).await? {
        return Ok(existing);
    }

    let product = learning_product::get_product_row(pool, cohort_row.product_id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("cohort references a missing learning_product")))?;

    // group: the product's own capacity. private: always exactly 1 seat
    // (learning_products' CHECK guarantees capacity is null for
    // private, so 1 is the type's own invariant, not a fallback).
    let limit = if product.r#type == "group" { product.capacity.unwrap_or(0) as i64 } else { 1 };
    let active_count = count_active_by_cohort(pool, cohort_id).await?;
    if active_count >= limit {
        return Err(AppError::UnprocessableEntity("cohort_full", "this cohort has no open seats left".to_string()));
    }

    // P23-002 — package products hand the student a fixed session
    // budget at enrollment time.
    let sessions_remaining = if product.delivery_mode == "package" { product.session_count } else { None };

    let inserted = sqlx::query_as!(
        EnrollmentResponse,
        r#"insert into enrollments (cohort_id, student_id, sessions_remaining) values ($1, $2, $3)
           on conflict (cohort_id, student_id) do nothing
           returning id, cohort_id, student_id, status, enrolled_at, sessions_remaining"#,
        cohort_id,
        ctx.user_id,
        sessions_remaining,
    )
    .fetch_optional(pool)
    .await?;

    match inserted {
        Some(row) => Ok(row),
        None => find_by_cohort_and_student(pool, cohort_id, ctx.user_id)
            .await?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("enrollments row missing after insert/lookup"))),
    }
}

async fn list_by_cohort_with_student(pool: &PgPool, cohort_id: Uuid) -> Result<Vec<StudentRowResponse>, AppError> {
    let rows = sqlx::query!(
        r#"select e.id, e.cohort_id, e.student_id, e.status, e.enrolled_at, e.sessions_remaining, u.name, u.email
           from enrollments e inner join users u on u.id = e.student_id
           where e.cohort_id = $1"#,
        cohort_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| StudentRowResponse {
            enrollment: EnrollmentResponse {
                id: r.id,
                cohort_id: r.cohort_id,
                student_id: r.student_id,
                status: r.status,
                enrolled_at: r.enrolled_at,
                sessions_remaining: r.sessions_remaining,
            },
            student_name: r.name,
            student_email: r.email,
        })
        .collect())
}

// GET /cohorts/{id}/students — 3-branch visibility: manager sees full
// roster; an enrolled student sees only their own row; anyone else 403.
pub async fn list_cohort_students(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<StudentListResponse, AppError> {
    let (_cohort, product) = cohort::find_product_for_cohort_id(pool, cohort_id).await?;

    if can_manage_cohorts(pool, ctx, &product).await? {
        return Ok(StudentListResponse { items: list_by_cohort_with_student(pool, cohort_id).await? });
    }

    let own = find_by_cohort_and_student(pool, cohort_id, ctx.user_id).await?;
    if let Some(own) = own {
        let student = sqlx::query!(r#"select name, email from users where id = $1"#, ctx.user_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("enrollment references a missing user")))?;
        return Ok(StudentListResponse { items: vec![StudentRowResponse { enrollment: own, student_name: student.name, student_email: student.email }] });
    }
    Err(AppError::Forbidden)
}

// POST /cohorts/{id}/enrollments/{enrollment_id}/complete
pub async fn complete_enrollment(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid, enrollment_id: Uuid) -> Result<EnrollmentResponse, AppError> {
    let (_cohort, product) = cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }

    let enrollment = find_by_id(pool, enrollment_id).await?.ok_or(AppError::NotFound("enrollment_not_found"))?;
    if enrollment.cohort_id != cohort_id {
        return Err(AppError::NotFound("enrollment_not_found"));
    }
    if enrollment.status == "completed" {
        return Ok(enrollment);
    }
    if enrollment.status != "active" {
        return Err(AppError::UnprocessableEntity("enrollment_not_active", "only an active (paid) enrollment can be marked completed".to_string()));
    }
    set_status(pool, enrollment_id, "completed").await
}

// GET /me/enrollments
pub async fn list_my_enrollments(pool: &PgPool, ctx: &AuthContext) -> Result<MyEnrollmentListResponse, AppError> {
    let rows = sqlx::query_as!(
        MyEnrollmentResponse,
        r#"select e.id, e.cohort_id, c.name as cohort_name, c.starts_at as cohort_starts_at,
                  lp.id as product_id, lp.title as product_title, lp.type as product_type,
                  lp.delivery_mode as product_delivery_mode, lp.tutor_id as tutor_id,
                  e.status, o.status as order_status, e.enrolled_at, e.sessions_remaining
           from enrollments e
           inner join cohorts c on c.id = e.cohort_id
           inner join learning_products lp on lp.id = c.product_id
           left join orders o on o.enrollment_id = e.id
           where e.student_id = $1
           order by e.enrolled_at desc"#,
        ctx.user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(MyEnrollmentListResponse { items: rows })
}

pub struct CancelResult {
    pub enrollment: EnrollmentResponse,
    pub refund_amount_idr: i64,
}

// POST /enrollments/{id}/cancel
pub async fn cancel_enrollment(pool: &PgPool, ctx: &AuthContext, enrollment_id: Uuid) -> Result<CancelResult, AppError> {
    let enrollment = find_by_id(pool, enrollment_id).await?.ok_or(AppError::NotFound("enrollment_not_found"))?;
    if enrollment.student_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    if enrollment.status == "cancelled" {
        return Ok(CancelResult { enrollment, refund_amount_idr: 0 });
    }

    let cohort_row = cohort::find_by_id(pool, enrollment.cohort_id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("enrollment references a missing cohort")))?;

    let order = crate::services::order::find_by_enrollment_id(pool, enrollment_id).await?;
    let mut refund_amount_idr = 0i64;

    if let Some(order) = &order {
        if order.status == "paid" {
            let fraction = refund_fraction(cohort_row.starts_at, Utc::now());
            if fraction > 0.0 {
                refund_amount_idr = (order.amount_idr as f64 * fraction).round() as i64;
                crate::services::wallet::record_refund(pool, ctx.user_id, refund_amount_idr, &format!("order:{}", order.id)).await?;
                crate::services::order::set_status(pool, order.id, "refunded").await?;
            }
            // fraction === 0: order stays 'paid' — no money moved.
        } else if order.status == "pending" {
            crate::services::order::set_status(pool, order.id, "failed").await?;
        }
    }

    let updated = cancel(pool, enrollment_id, "student").await?;
    Ok(CancelResult { enrollment: updated, refund_amount_idr })
}

// P12-001 — tutor-cancel is always a full refund regardless of timing
// (student is never at fault for a tutor's cancellation) — deliberately
// a DIFFERENT policy from refund_fraction, not a reuse of it.
pub async fn tutor_cancel_enrollment(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid, enrollment_id: Uuid) -> Result<CancelResult, AppError> {
    let enrollment = find_by_id(pool, enrollment_id).await?.ok_or(AppError::NotFound("enrollment_not_found"))?;
    if enrollment.cohort_id != cohort_id {
        return Err(AppError::NotFound("enrollment_not_found"));
    }
    let (_cohort, product) = cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }
    if enrollment.status == "cancelled" {
        return Ok(CancelResult { enrollment, refund_amount_idr: 0 });
    }

    let order = crate::services::order::find_by_enrollment_id(pool, enrollment_id).await?;
    let mut refund_amount_idr = 0i64;
    if let Some(order) = &order {
        if order.status == "paid" {
            refund_amount_idr = order.amount_idr; // always 100%
            crate::services::wallet::record_refund(pool, enrollment.student_id, refund_amount_idr, &format!("order:{}", order.id)).await?;
            crate::services::order::set_status(pool, order.id, "refunded").await?;
        } else if order.status == "pending" {
            crate::services::order::set_status(pool, order.id, "failed").await?;
        }
    }

    let updated = cancel(pool, enrollment_id, "tutor").await?;
    Ok(CancelResult { enrollment: updated, refund_amount_idr })
}

// Cancellation policy (§8.14, tunable default). No scheduled starts_at
// yet defaults to a FULL refund — no basis to withhold money when the
// tutor hasn't even committed to a start time.
fn refund_fraction(starts_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> f64 {
    let Some(starts_at) = starts_at else { return 1.0 };
    let hours_until_start = (starts_at - now).num_seconds() as f64 / 3600.0;
    if hours_until_start > 24.0 {
        1.0
    } else if hours_until_start >= 6.0 {
        0.5
    } else {
        0.0
    }
}
