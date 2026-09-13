// Whether a learner has finished a module item, their best score on it,
// and a tutor's approval — the facts every access gate and attendance
// guard is evaluated against (migrations/0045).
//
//   article → completed when the learner finishes reading it
//   quiz    → completed on submit; best_score is the highest attempt
//             score (a manually graded attempt updates it once graded)

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{is_allowed, Action, Resource};

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub completed: bool,
    pub best_score: Option<f64>,
    pub approved: bool,
}

/// Records a finished item. Never un-finishes one, and never lowers a
/// best score — retaking a quiz badly does not re-lock what it unlocked.
///
/// P39-004 (ADR-0013 L1) — also emits `module_item_completed`, every
/// time this is called (not only on the FIRST completion): a repeat
/// completion is still a real thing that happened, and the event log's
/// job is to keep every occurrence, unlike this table's own upsert
/// which only keeps the best one. `source` is the caller's own
/// classification (ADR-0013 §1.5) — a quiz completing under a
/// proctored sitting and a learner finishing reading an article are
/// both "completion", but not the same kind of behaviour.
pub async fn record_completion(pool: &PgPool, user_id: Uuid, item_id: Uuid, score: Option<f64>, source: &'static str) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into module_item_progress (user_id, item_id, completed_at, best_score)
           values ($1, $2, now(), $3)
           on conflict (user_id, item_id) do update
              set completed_at = coalesce(module_item_progress.completed_at, excluded.completed_at),
                  best_score = greatest(module_item_progress.best_score, excluded.best_score),
                  updated_at = now()"#,
        user_id,
        item_id,
        score,
    )
    .execute(pool)
    .await?;

    let mut event = crate::services::learning_event::NewLearningEvent::server("module_item_completed", "module_item", item_id, serde_json::json!({"score": score}), source);
    event.module_item_id = Some(item_id);
    crate::services::learning_event::record(pool, user_id, crate::services::learning_event::EventChannel::Server, event).await?;

    Ok(())
}

/// Progress of one learner on the given items. A Diamond skip
/// (item_completion_overrides) counts as finished and passed — that is
/// what the learner paid to skip.
pub async fn for_user(pool: &PgPool, user_id: Uuid, item_ids: &[Uuid]) -> Result<HashMap<Uuid, Progress>, AppError> {
    let mut out: HashMap<Uuid, Progress> = HashMap::new();
    if item_ids.is_empty() {
        return Ok(out);
    }
    let rows = sqlx::query!(
        r#"select item_id, completed_at, best_score, approved_at from module_item_progress where user_id = $1 and item_id = any($2)"#,
        user_id,
        item_ids,
    )
    .fetch_all(pool)
    .await?;
    for r in rows {
        out.insert(r.item_id, Progress { completed: r.completed_at.is_some(), best_score: r.best_score, approved: r.approved_at.is_some() });
    }
    let skipped = sqlx::query_scalar!(r#"select item_id from item_completion_overrides where user_id = $1 and item_id = any($2)"#, user_id, item_ids)
        .fetch_all(pool)
        .await?;
    for id in skipped {
        let p = out.entry(id).or_default();
        p.completed = true;
        p.best_score = Some(p.best_score.unwrap_or(0.0).max(100.0));
    }
    Ok(out)
}

/// Authors, plus the teacher of any class that studies this item's
/// module (directly, or through the class's program).
pub async fn can_review_item(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<bool, AppError> {
    if is_allowed(ctx.role.as_deref(), Resource::ModuleItem, Action::Create) {
        return Ok(true);
    }
    let teaches = sqlx::query_scalar!(
        r#"select exists(
             select 1 from classes c
             join module_items i on i.id = $2
             where c.teacher_id = $1
               and (c.module_id = i.module_id
                    or c.program_id in (select pm.program_id from program_modules pm where pm.module_id = i.module_id))
           ) as "exists!""#,
        ctx.user_id,
        item_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(teaches)
}

#[derive(Debug, serde::Serialize)]
pub struct ProgressRow {
    pub user_id: Uuid,
    pub name: String,
    pub email: String,
    pub completed_at: Option<DateTime<Utc>>,
    pub best_score: Option<f64>,
    pub approved_at: Option<DateTime<Utc>>,
}

// GET /module-items/{id}/progress — who has finished this item, for the
// tutor's "Perlu approve tutor" list.
pub async fn list_for_item(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<Vec<ProgressRow>, AppError> {
    if !can_review_item(pool, ctx, item_id).await? {
        return Err(AppError::Forbidden);
    }
    let rows = sqlx::query_as!(
        ProgressRow,
        r#"select p.user_id, u.name, u.email, p.completed_at, p.best_score, p.approved_at
           from module_item_progress p join users u on u.id = p.user_id
           where p.item_id = $1
           order by p.approved_at is not null, p.completed_at desc nulls last"#,
        item_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// POST /module-items/{id}/progress/{user_id}/approve — a tutor's verdict
// for a "dinilai tutor" gate. `approved: false` withdraws it.
pub async fn set_approval(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, student_id: Uuid, approved: bool) -> Result<(), AppError> {
    if !can_review_item(pool, ctx, item_id).await? {
        return Err(AppError::Forbidden);
    }
    let (at, by) = if approved { (Some(Utc::now()), Some(ctx.user_id)) } else { (None, None) };
    sqlx::query!(
        r#"insert into module_item_progress (user_id, item_id, approved_at, approved_by)
           values ($1, $2, $3, $4)
           on conflict (user_id, item_id) do update
              set approved_at = excluded.approved_at, approved_by = excluded.approved_by, updated_at = now()"#,
        student_id,
        item_id,
        at,
        by,
    )
    .execute(pool)
    .await?;
    Ok(())
}
