use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::cohort::{self, can_manage_cohorts};

#[derive(Debug, serde::Serialize, Clone)]
pub struct AssignmentResponse {
    pub id: Uuid,
    pub cohort_id: Uuid,
    pub title: String,
    pub description: String,
    pub deadline: Option<DateTime<Utc>>,
}

#[derive(Debug, serde::Serialize)]
pub struct AssignmentListResponse {
    pub items: Vec<AssignmentResponse>,
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<AssignmentResponse>, AppError> {
    let row = sqlx::query_as!(AssignmentResponse, r#"select id, cohort_id, title, description, deadline from assignments where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

async fn load_cohort_and_product(pool: &PgPool, cohort_id: Uuid) -> Result<crate::services::learning_product::ProductResponse, AppError> {
    let (_cohort, product) = cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    Ok(product)
}

// POST /cohorts/{id}/assignments
pub async fn create_assignment(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid, title: &str, description: &str, deadline: Option<DateTime<Utc>>) -> Result<AssignmentResponse, AppError> {
    let product = load_cohort_and_product(pool, cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }
    let row = sqlx::query_as!(
        AssignmentResponse,
        r#"insert into assignments (cohort_id, title, description, deadline) values ($1, $2, $3, $4)
           returning id, cohort_id, title, description, deadline"#,
        cohort_id,
        title,
        description,
        deadline,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// GET /cohorts/{id}/assignments — manager OR any enrolled student (any
// enrollment status, not just active).
pub async fn list_assignments(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<AssignmentListResponse, AppError> {
    let product = load_cohort_and_product(pool, cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        let enrollment = sqlx::query_scalar!(r#"select id from enrollments where cohort_id = $1 and student_id = $2"#, cohort_id, ctx.user_id)
            .fetch_optional(pool)
            .await?;
        if enrollment.is_none() {
            return Err(AppError::Forbidden);
        }
    }
    let rows = sqlx::query_as!(AssignmentResponse, r#"select id, cohort_id, title, description, deadline from assignments where cohort_id = $1"#, cohort_id)
        .fetch_all(pool)
        .await?;
    Ok(AssignmentListResponse { items: rows })
}

fn compute_late(assignment: &AssignmentResponse, submitted_at: DateTime<Utc>) -> bool {
    assignment.deadline.map(|d| submitted_at > d).unwrap_or(false)
}

struct SubmissionRow {
    id: Uuid,
    assignment_id: Uuid,
    student_id: Uuid,
    content: String,
    submitted_at: DateTime<Utc>,
    score: Option<f64>,
    feedback: Option<String>,
    graded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, serde::Serialize)]
pub struct SubmissionResponse {
    pub id: Uuid,
    pub assignment_id: Uuid,
    pub student_id: Uuid,
    pub content: String,
    pub submitted_at: DateTime<Utc>,
    pub late: bool,
    pub score: Option<f64>,
    pub feedback: Option<String>,
    pub graded_at: Option<DateTime<Utc>>,
}

fn to_response(row: SubmissionRow, late: bool) -> SubmissionResponse {
    SubmissionResponse {
        id: row.id,
        assignment_id: row.assignment_id,
        student_id: row.student_id,
        content: row.content,
        submitted_at: row.submitted_at,
        late,
        score: row.score,
        feedback: row.feedback,
        graded_at: row.graded_at,
    }
}

async fn find_submission_by_assignment_and_student(pool: &PgPool, assignment_id: Uuid, student_id: Uuid) -> Result<Option<SubmissionRow>, AppError> {
    let row = sqlx::query_as!(
        SubmissionRow,
        r#"select id, assignment_id, student_id, content, submitted_at, score, feedback, graded_at
           from assignment_submissions where assignment_id = $1 and student_id = $2"#,
        assignment_id,
        student_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// POST /assignments/{id}/submissions — self-service; resubmitting
// BEFORE grading updates the same row, AFTER grading is rejected (409).
pub async fn submit_assignment(pool: &PgPool, ctx: &AuthContext, assignment_id: Uuid, content: &str) -> Result<SubmissionResponse, AppError> {
    let assignment = find_by_id(pool, assignment_id).await?.ok_or(AppError::NotFound("assignment_not_found"))?;

    let enrollment = sqlx::query_scalar!(r#"select id from enrollments where cohort_id = $1 and student_id = $2"#, assignment.cohort_id, ctx.user_id)
        .fetch_optional(pool)
        .await?;
    if enrollment.is_none() {
        return Err(AppError::Forbidden);
    }

    let existing = find_submission_by_assignment_and_student(pool, assignment_id, ctx.user_id).await?;
    let row = match existing {
        None => {
            sqlx::query_as!(
                SubmissionRow,
                r#"insert into assignment_submissions (assignment_id, student_id, content) values ($1, $2, $3)
                   returning id, assignment_id, student_id, content, submitted_at, score, feedback, graded_at"#,
                assignment_id,
                ctx.user_id,
                content,
            )
            .fetch_one(pool)
            .await?
        }
        Some(existing) if existing.graded_at.is_some() => return Err(AppError::Conflict("submission_already_graded")),
        Some(existing) => {
            sqlx::query_as!(
                SubmissionRow,
                r#"update assignment_submissions set content = $2, submitted_at = now() where id = $1
                   returning id, assignment_id, student_id, content, submitted_at, score, feedback, graded_at"#,
                existing.id,
                content,
            )
            .fetch_one(pool)
            .await?
        }
    };
    let late = compute_late(&assignment, row.submitted_at);
    Ok(to_response(row, late))
}

#[derive(Debug, serde::Serialize)]
pub struct SubmissionRowResponse {
    #[serde(flatten)]
    pub submission: SubmissionResponse,
    pub student_name: String,
}

#[derive(Debug, serde::Serialize)]
pub struct SubmissionListResponse {
    pub items: Vec<SubmissionRowResponse>,
}

// GET /assignments/{id}/submissions — manager sees all, student sees
// own (or empty list if not yet submitted — enrolled-but-no-submission
// is not a 404).
pub async fn list_submissions(pool: &PgPool, ctx: &AuthContext, assignment_id: Uuid) -> Result<SubmissionListResponse, AppError> {
    let assignment = find_by_id(pool, assignment_id).await?.ok_or(AppError::NotFound("assignment_not_found"))?;
    let product = load_cohort_and_product(pool, assignment.cohort_id).await?;

    if can_manage_cohorts(pool, ctx, &product).await? {
        let rows = sqlx::query!(
            r#"select s.id, s.assignment_id, s.student_id, s.content, s.submitted_at, s.score, s.feedback, s.graded_at, u.name
               from assignment_submissions s inner join users u on u.id = s.student_id
               where s.assignment_id = $1"#,
            assignment_id,
        )
        .fetch_all(pool)
        .await?;
        let items = rows
            .into_iter()
            .map(|r| {
                let late = assignment.deadline.map(|d| r.submitted_at > d).unwrap_or(false);
                SubmissionRowResponse {
                    submission: SubmissionResponse {
                        id: r.id,
                        assignment_id: r.assignment_id,
                        student_id: r.student_id,
                        content: r.content,
                        submitted_at: r.submitted_at,
                        late,
                        score: r.score,
                        feedback: r.feedback,
                        graded_at: r.graded_at,
                    },
                    student_name: r.name,
                }
            })
            .collect();
        return Ok(SubmissionListResponse { items });
    }

    let enrollment = sqlx::query_scalar!(r#"select id from enrollments where cohort_id = $1 and student_id = $2"#, assignment.cohort_id, ctx.user_id)
        .fetch_optional(pool)
        .await?;
    if enrollment.is_none() {
        return Err(AppError::Forbidden);
    }

    let own = find_submission_by_assignment_and_student(pool, assignment_id, ctx.user_id).await?;
    let Some(own) = own else { return Ok(SubmissionListResponse { items: vec![] }) };
    let student = sqlx::query!(r#"select name from users where id = $1"#, ctx.user_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("submission references a missing user")))?;
    let late = compute_late(&assignment, own.submitted_at);
    Ok(SubmissionListResponse { items: vec![SubmissionRowResponse { submission: to_response(own, late), student_name: student.name }] })
}

// POST /submissions/{id}/grade — no score-range validation whatsoever,
// matching Bun exactly (no CHECK constraint on the column either).
// Regrading an already-graded submission simply overwrites it.
pub async fn grade_submission(pool: &PgPool, ctx: &AuthContext, submission_id: Uuid, score: f64, feedback: Option<&str>) -> Result<SubmissionResponse, AppError> {
    let submission = sqlx::query_as!(
        SubmissionRow,
        r#"select id, assignment_id, student_id, content, submitted_at, score, feedback, graded_at
           from assignment_submissions where id = $1"#,
        submission_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("submission_not_found"))?;

    let assignment = find_by_id(pool, submission.assignment_id)
        .await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("submission references a missing assignment")))?;
    let product = load_cohort_and_product(pool, assignment.cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }

    let graded = sqlx::query_as!(
        SubmissionRow,
        r#"update assignment_submissions set score = $2, feedback = $3, graded_at = now(), graded_by = $4 where id = $1
           returning id, assignment_id, student_id, content, submitted_at, score, feedback, graded_at"#,
        submission_id,
        score,
        feedback,
        ctx.user_id,
    )
    .fetch_one(pool)
    .await?;
    let late = compute_late(&assignment, graded.submitted_at);
    Ok(to_response(graded, late))
}
