use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use rand::RngCore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::cohort::can_manage_cohorts;
use crate::Config;

fn generate_certificate_code() -> String {
    let mut bytes = [0u8; 9];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn estimate_cefr(average_score: f64) -> &'static str {
    if average_score < 20.0 {
        "A1"
    } else if average_score < 40.0 {
        "A2"
    } else if average_score < 60.0 {
        "B1"
    } else if average_score < 75.0 {
        "B2"
    } else if average_score < 90.0 {
        "C1"
    } else {
        "C2"
    }
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct CertificateResponse {
    pub id: Uuid,
    pub enrollment_id: Uuid,
    pub certificate_code: String,
    pub issued_at: DateTime<Utc>,
    pub completion_percent: i32,
    pub estimated_cefr: Option<String>,
    pub skill_summary: serde_json::Value,
}

pub async fn find_by_enrollment_id(pool: &PgPool, enrollment_id: Uuid) -> Result<Option<CertificateResponse>, AppError> {
    let row = sqlx::query_as!(
        CertificateResponse,
        r#"select id, enrollment_id, certificate_code, issued_at, completion_percent, estimated_cefr, skill_summary
           from certificates where enrollment_id = $1"#,
        enrollment_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn find_average_score(pool: &PgPool, user_id: Uuid, confidence_threshold: f64) -> Result<Option<f64>, AppError> {
    let average = sqlx::query_scalar!(r#"select avg(score) as "average" from masteries where user_id = $1 and confidence >= $2"#, user_id, confidence_threshold)
        .fetch_one(pool)
        .await?;
    Ok(average)
}

async fn find_average_score_by_skill_category(pool: &PgPool, user_id: Uuid, confidence_threshold: f64) -> Result<serde_json::Value, AppError> {
    let rows = sqlx::query!(
        r#"select q.skill_category as "skill_category!", avg(m.score) as "average!"
           from masteries m
           inner join question_concepts qc on qc.concept_id = m.concept_id
           inner join questions q on q.id = qc.question_id
           where m.user_id = $1 and m.confidence >= $2 and q.skill_category is not null
           group by q.skill_category"#,
        user_id,
        confidence_threshold,
    )
    .fetch_all(pool)
    .await?;
    let map: serde_json::Map<String, serde_json::Value> =
        rows.into_iter().map(|r| (r.skill_category, serde_json::json!(r.average.round() as i64))).collect();
    Ok(serde_json::Value::Object(map))
}

// POST /cohorts/{id}/enrollments/{enrollment_id}/certificate
pub async fn issue_certificate(pool: &PgPool, config: &Config, ctx: &AuthContext, cohort_id: Uuid, enrollment_id: Uuid) -> Result<CertificateResponse, AppError> {
    let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }

    let enrollment = crate::services::enrollment::find_by_id(pool, enrollment_id).await?.ok_or(AppError::NotFound("enrollment_not_found"))?;
    if enrollment.cohort_id != cohort_id {
        return Err(AppError::NotFound("enrollment_not_found"));
    }
    if enrollment.status != "completed" {
        return Err(AppError::UnprocessableEntity("enrollment_not_completed", "certificates can only be issued for a completed enrollment".to_string()));
    }

    if find_by_enrollment_id(pool, enrollment_id).await?.is_some() {
        return Err(AppError::Conflict("certificate_already_issued"));
    }

    let average_score = find_average_score(pool, enrollment.student_id, config.mastery_confidence_threshold).await?;
    let skill_summary = find_average_score_by_skill_category(pool, enrollment.student_id, config.mastery_confidence_threshold).await?;
    let estimated_cefr = average_score.map(estimate_cefr);

    let row = sqlx::query_as!(
        CertificateResponse,
        r#"insert into certificates (enrollment_id, certificate_code, completion_percent, estimated_cefr, skill_summary)
           values ($1, $2, 100, $3, $4)
           returning id, enrollment_id, certificate_code, issued_at, completion_percent, estimated_cefr, skill_summary"#,
        enrollment_id,
        generate_certificate_code(),
        estimated_cefr,
        skill_summary,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /enrollments/{id}/certificate — "not issued yet" is legitimate,
// not an error.
pub async fn get_certificate_for_enrollment(pool: &PgPool, ctx: &AuthContext, enrollment_id: Uuid) -> Result<Option<CertificateResponse>, AppError> {
    let enrollment = crate::services::enrollment::find_by_id(pool, enrollment_id).await?.ok_or(AppError::NotFound("enrollment_not_found"))?;

    if enrollment.student_id != ctx.user_id {
        let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, enrollment.cohort_id).await?;
        if !can_manage_cohorts(pool, ctx, &product).await? {
            return Err(AppError::Forbidden);
        }
    }
    find_by_enrollment_id(pool, enrollment_id).await
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum VerifyResponse {
    Valid { valid: bool, student_name: String, course_title: String, completion_date: DateTime<Utc> },
    Invalid { valid: bool },
}

// GET /certificates/{code}/verify — PUBLIC, always HTTP 200. Unknown
// code -> {"valid": false}, never 404 (a caller can't distinguish "never
// issued" from any other rejection reason).
pub async fn verify(pool: &PgPool, code: &str) -> Result<VerifyResponse, AppError> {
    let row = sqlx::query!(
        r#"select u.name as student_name, lp.title as course_title, c.issued_at as completion_date
           from certificates c
           inner join enrollments e on e.id = c.enrollment_id
           inner join users u on u.id = e.student_id
           inner join cohorts co on co.id = e.cohort_id
           inner join learning_products lp on lp.id = co.product_id
           where c.certificate_code = $1"#,
        code,
    )
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some(r) => VerifyResponse::Valid { valid: true, student_name: r.student_name, course_title: r.course_title, completion_date: r.completion_date },
        None => VerifyResponse::Invalid { valid: false },
    })
}
