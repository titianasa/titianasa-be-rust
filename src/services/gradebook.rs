use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::cohort::can_manage_cohorts;

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    Some((values.iter().sum::<f64>() / values.len() as f64 * 10.0).round() / 10.0)
}

#[derive(Debug, serde::Serialize)]
pub struct GradebookRowResponse {
    pub student_id: Uuid,
    pub student_name: String,
    pub attendance_percent: Option<f64>,
    pub assignment_average: Option<f64>,
    pub overall: Option<f64>,
}

#[derive(Debug, serde::Serialize)]
pub struct GradebookResponse {
    pub items: Vec<GradebookRowResponse>,
}

// GET /cohorts/{id}/gradebook — manager-only, no student branch at all.
pub async fn get_gradebook(pool: &PgPool, ctx: &AuthContext, cohort_id: Uuid) -> Result<GradebookResponse, AppError> {
    let (_cohort, product) = crate::services::cohort::find_product_for_cohort_id(pool, cohort_id).await?;
    if !can_manage_cohorts(pool, ctx, &product).await? {
        return Err(AppError::Forbidden);
    }

    let roster = sqlx::query!(
        r#"select e.student_id, e.status, u.name from enrollments e
           inner join users u on u.id = e.student_id
           where e.cohort_id = $1 and e.status in ('active', 'completed')"#,
        cohort_id,
    )
    .fetch_all(pool)
    .await?;

    // R8 dependency: attendance_records isn't ported yet — this single
    // trivial SELECT is ported early rather than deferring the whole
    // gradebook; if the table is genuinely empty for now, every student
    // just gets attendance_percent: null, and `overall` already falls
    // back to assignment_average alone (see below).
    let attendance = sqlx::query!(r#"select student_id, status from attendance_records where cohort_id = $1"#, cohort_id).fetch_all(pool).await?;

    let mut attendance_by_student: std::collections::HashMap<Uuid, (i64, i64)> = std::collections::HashMap::new();
    for record in attendance {
        let entry = attendance_by_student.entry(record.student_id).or_insert((0, 0));
        entry.1 += 1;
        if record.status == "present" {
            entry.0 += 1;
        }
    }

    let graded_submissions = sqlx::query!(
        r#"select s.student_id, s.score as "score!" from assignment_submissions s
           inner join assignments a on a.id = s.assignment_id
           where a.cohort_id = $1 and s.score is not null"#,
        cohort_id,
    )
    .fetch_all(pool)
    .await?;

    let mut scores_by_student: std::collections::HashMap<Uuid, Vec<f64>> = std::collections::HashMap::new();
    for submission in graded_submissions {
        scores_by_student.entry(submission.student_id).or_default().push(submission.score);
    }

    let items = roster
        .into_iter()
        .map(|row| {
            let student_id = row.student_id;
            let attendance_percent = attendance_by_student.get(&student_id).and_then(|(present, total)| {
                if *total > 0 {
                    Some((*present as f64 / *total as f64 * 1000.0).round() / 10.0)
                } else {
                    None
                }
            });
            let assignment_average = average(scores_by_student.get(&student_id).map(|v| v.as_slice()).unwrap_or(&[]));

            let components: Vec<f64> = [attendance_percent, assignment_average].into_iter().flatten().collect();
            let overall = if components.is_empty() { None } else { average(&components) };

            GradebookRowResponse { student_id, student_name: row.name, attendance_percent, assignment_average, overall }
        })
        .collect();

    Ok(GradebookResponse { items })
}
