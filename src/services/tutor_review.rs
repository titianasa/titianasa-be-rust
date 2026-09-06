use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;

#[derive(Debug, serde::Serialize)]
pub struct ReviewResponse {
    pub id: Uuid,
    pub tutor_id: Uuid,
    pub student_id: Uuid,
    pub enrollment_id: Uuid,
    pub rating: i32,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}

// POST /tutors/{id}/reviews. Note (reproduced from Bun as-is): tutor_id
// comes from the URL path and is never cross-checked against the
// enrollment's actual tutor — a real gap in the source, kept for a
// faithful 1:1 port rather than silently fixed.
pub async fn submit_review(pool: &PgPool, ctx: &AuthContext, tutor_id: Uuid, enrollment_id: Uuid, rating: i32, comment: Option<&str>) -> Result<ReviewResponse, AppError> {
    if !(1..=5).contains(&rating) {
        return Err(AppError::UnprocessableEntity("invalid_rating", "rating must be an integer between 1 and 5".to_string()));
    }

    let enrollment = crate::services::enrollment::find_by_id(pool, enrollment_id).await?.ok_or(AppError::NotFound("enrollment_not_found"))?;
    if enrollment.student_id != ctx.user_id {
        return Err(AppError::Forbidden);
    }
    if enrollment.status != "completed" {
        return Err(AppError::UnprocessableEntity("enrollment_not_completed", "enrollment must be completed before it can be reviewed".to_string()));
    }

    let existing = sqlx::query_scalar!(r#"select id from tutor_reviews where enrollment_id = $1"#, enrollment_id).fetch_optional(pool).await?;
    if existing.is_some() {
        return Err(AppError::Conflict("already_reviewed"));
    }

    let row = sqlx::query_as!(
        ReviewResponse,
        r#"insert into tutor_reviews (tutor_id, student_id, enrollment_id, rating, comment) values ($1, $2, $3, $4, $5)
           returning id, tutor_id, student_id, enrollment_id, rating, comment, created_at"#,
        tutor_id,
        ctx.user_id,
        enrollment_id,
        rating,
        comment,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

#[derive(Debug, serde::Serialize)]
pub struct ReputationResponse {
    pub average_rating: Option<f64>,
    pub review_count: i64,
    pub student_count: i64,
    pub lesson_count: i64,
    pub completion_rate: Option<f64>,
    pub cancellation_rate: Option<f64>,
    pub avg_response_minutes: Option<i64>,
    pub response_under_1h_rate: Option<f64>,
}

// P22-002 — closes P12-004's explicitly-deferred "response time <1
// jam". For each of the tutor's conversations, walks messages
// oldest-first and times the gap from the FIRST unanswered student
// message to the next tutor message after it (a burst of student
// messages before any reply only counts once — the moment the tutor
// actually responded).
async fn compute_response_time_stats(pool: &PgPool, tutor_id: Uuid) -> Result<(Option<i64>, Option<f64>), AppError> {
    let convos = sqlx::query!(r#"select id, student_id, tutor_id from conversations where tutor_id = $1"#, tutor_id).fetch_all(pool).await?;
    if convos.is_empty() {
        return Ok((None, None));
    }

    let convo_ids: Vec<Uuid> = convos.iter().map(|c| c.id).collect();
    let rows = sqlx::query!(
        r#"select conversation_id, sender_id, created_at from messages where conversation_id = any($1) order by conversation_id, created_at asc"#,
        &convo_ids,
    )
    .fetch_all(pool)
    .await?;

    let mut by_conversation: std::collections::HashMap<Uuid, Vec<(Uuid, DateTime<Utc>)>> = std::collections::HashMap::new();
    for row in &rows {
        by_conversation.entry(row.conversation_id).or_default().push((row.sender_id, row.created_at));
    }

    let mut delta_minutes: Vec<f64> = Vec::new();
    for convo in &convos {
        let Some(convo_rows) = by_conversation.get(&convo.id) else { continue };
        let mut pending_student_message_at: Option<DateTime<Utc>> = None;
        for (sender_id, created_at) in convo_rows {
            if *sender_id == convo.student_id {
                if pending_student_message_at.is_none() {
                    pending_student_message_at = Some(*created_at);
                }
            } else if *sender_id == convo.tutor_id {
                if let Some(pending_at) = pending_student_message_at {
                    delta_minutes.push((*created_at - pending_at).num_seconds() as f64 / 60.0);
                    pending_student_message_at = None;
                }
            }
        }
    }

    if delta_minutes.is_empty() {
        return Ok((None, None));
    }
    let avg = delta_minutes.iter().sum::<f64>() / delta_minutes.len() as f64;
    let under_1h = delta_minutes.iter().filter(|d| **d <= 60.0).count() as f64 / delta_minutes.len() as f64;
    Ok((Some(avg.round() as i64), Some((under_1h * 100.0).round() / 100.0)))
}

// GET /tutors/{id}/reputation — fully public, no permission check at
// all (matches Bun exactly, despite api-contract.md's stale claim
// otherwise).
pub async fn get_reputation(pool: &PgPool, tutor_id: Uuid) -> Result<ReputationResponse, AppError> {
    let review_stats = sqlx::query!(r#"select avg(rating)::float8 as "avg", count(*) as "count!" from tutor_reviews where tutor_id = $1"#, tutor_id)
        .fetch_one(pool)
        .await?;

    let enrollment_rows = sqlx::query!(
        r#"select e.student_id, e.status, e.cancelled_by from enrollments e
           inner join cohorts c on c.id = e.cohort_id
           inner join learning_products lp on lp.id = c.product_id
           where lp.tutor_id = $1"#,
        tutor_id,
    )
    .fetch_all(pool)
    .await?;

    let student_count = enrollment_rows.iter().map(|r| r.student_id).collect::<std::collections::HashSet<_>>().len() as i64;
    let completed = enrollment_rows.iter().filter(|r| r.status == "completed").count() as i64;
    let cancelled = enrollment_rows.iter().filter(|r| r.status == "cancelled").count() as i64;
    let completion_rate = if completed + cancelled > 0 { Some(completed as f64 / (completed + cancelled) as f64) } else { None };

    let tutor_cancelled_count = enrollment_rows.iter().filter(|r| r.cancelled_by.as_deref() == Some("tutor")).count() as i64;
    let cancellation_rate = if !enrollment_rows.is_empty() { Some(tutor_cancelled_count as f64 / enrollment_rows.len() as f64) } else { None };

    // lesson_count = 0 until R8 (attendance_records) exists — matches
    // what count(*) naturally yields on an absent/empty set anyway.
    let lesson_count = sqlx::query_scalar!(
        r#"select count(*) as "count!" from attendance_records ar
           inner join cohorts c on c.id = ar.cohort_id
           inner join learning_products lp on lp.id = c.product_id
           where lp.tutor_id = $1 and ar.status = 'present'"#,
        tutor_id,
    )
    .fetch_one(pool)
    .await?;

    let (avg_response_minutes, response_under_1h_rate) = compute_response_time_stats(pool, tutor_id).await?;

    Ok(ReputationResponse {
        average_rating: review_stats.avg.map(|a| (a * 10.0).round() / 10.0),
        review_count: review_stats.count,
        student_count,
        lesson_count,
        completion_rate,
        cancellation_rate,
        avg_response_minutes,
        response_under_1h_rate,
    })
}
