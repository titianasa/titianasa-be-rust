// Flags on content that looks wrong, and the admin tickets they collect
// into (migrations/0057). Learners send reports; Admin Pusat works
// tickets. A reporter only ever gets "terima kasih" back — deliberately:
// the product decision was no follow-up to the reporter.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::content_context::{self, ItemContext};
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::{admin_audit, module_item};

pub const CATEGORIES: [&str; 7] = ["typo", "wrong_answer_key", "bad_choices", "unclear", "incorrect_content", "display_broken", "other"];
const TARGETS: [&str; 3] = ["question", "checkpoint_question", "section"];
/// Enough for a learner who genuinely finds a bad bab; a stop for anyone
/// trying to bury the queue.
const DAILY_REPORT_LIMIT: i64 = 30;
const MAX_CONTEXT_BYTES: usize = 8_000;
const STATUSES: [&str; 4] = ["open", "in_review", "resolved", "rejected"];

#[derive(Debug, Deserialize)]
pub struct CreateReportRequest {
    /// The item the learner was in (a pooled Latihan, a preview, a Modul Belajar).
    pub module_item_id: Uuid,
    pub target_type: String,
    pub content_uid: String,
    pub category: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub context: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct CreateReportResponse {
    pub received: bool,
}

fn invalid(code: &'static str, detail: &str) -> AppError {
    AppError::UnprocessableEntity(code, detail.to_string())
}

fn find_question<'a>(config: Option<&'a Value>, uid: &str) -> Option<(&'a Value, &'a Value)> {
    config?.get("question_groups")?.as_array()?.iter().find_map(|group| {
        group.get("questions")?.as_array()?.iter().find(|q| q.get("uid").and_then(Value::as_str) == Some(uid)).map(|q| (group, q))
    })
}

fn find_section<'a>(plan: Option<&'a Value>, section_id: &str) -> Option<&'a Value> {
    plan?.get("sections")?.as_array()?.iter().find(|s| s.get("id").and_then(Value::as_str) == Some(section_id))
}

fn find_checkpoint_question<'a>(plan: Option<&'a Value>, uid: &str) -> Option<(&'a Value, &'a Value, &'a Value)> {
    plan?.get("sections")?.as_array()?.iter().find_map(|section| find_question(section.get("checkpoint"), uid).map(|(g, q)| (section, g, q)))
}

struct Owner {
    item_id: Uuid,
    version: Option<i32>,
}

/// Which item actually holds the reported content. A pooled Latihan
/// holds no questions — its questions live in the bank it draws from,
/// and that bank is where an author has to go to fix one.
async fn owner_of(pool: &PgPool, item_id: Uuid, target_type: &str, uid: &str) -> Result<Owner, AppError> {
    let row = sqlx::query!(r#"select quiz_config, lesson_plan, current_version from module_items where id = $1"#, item_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("module_item_not_found"))?;
    let found = match target_type {
        "question" => {
            if find_question(row.quiz_config.as_ref(), uid).is_some() {
                return Ok(Owner { item_id, version: row.current_version });
            }
            let source = row.quiz_config.as_ref().and_then(|c| c.pointer("/question_pool/source_item_id")).and_then(Value::as_str).and_then(|s| s.parse::<Uuid>().ok());
            if let Some(source) = source {
                let bank = sqlx::query!(r#"select quiz_config, current_version from module_items where id = $1"#, source).fetch_optional(pool).await?;
                if let Some(bank) = bank.filter(|b| find_question(b.quiz_config.as_ref(), uid).is_some()) {
                    return Ok(Owner { item_id: source, version: bank.current_version });
                }
            }
            false
        }
        "checkpoint_question" => find_checkpoint_question(row.lesson_plan.as_ref(), uid).is_some(),
        _ => find_section(row.lesson_plan.as_ref(), uid).is_some(),
    };
    if found {
        Ok(Owner { item_id, version: row.current_version })
    } else {
        Err(AppError::NotFound("content_not_found"))
    }
}

/// A question's stem, short — wherever it lives (a quiz bank or a
/// checkpoint pool). Shared with the heatmap's "soal tersulit".
pub(crate) async fn question_summary(pool: &PgPool, item_id: Uuid, uid: &str) -> Result<Option<String>, AppError> {
    let Some(row) = sqlx::query!(r#"select quiz_config, lesson_plan from module_items where id = $1"#, item_id).fetch_optional(pool).await? else {
        return Ok(None);
    };
    let question = find_question(row.quiz_config.as_ref(), uid).map(|(_, q)| q).or_else(|| find_checkpoint_question(row.lesson_plan.as_ref(), uid).map(|(_, _, q)| q));
    Ok(question.map(|q| excerpt(q.get("stem").or_else(|| q.get("text")).and_then(Value::as_str).unwrap_or(""), 140)))
}

// POST /content-reports
pub async fn create(pool: &PgPool, ctx: &AuthContext, req: CreateReportRequest) -> Result<CreateReportResponse, AppError> {
    if !TARGETS.contains(&req.target_type.as_str()) {
        return Err(invalid("invalid_target_type", "target_type tidak dikenal"));
    }
    if !CATEGORIES.contains(&req.category.as_str()) {
        return Err(invalid("invalid_category", "kategori laporan tidak dikenal"));
    }
    let message = req.message.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(|m| m.chars().take(1000).collect::<String>());
    if req.category == "other" && message.is_none() {
        return Err(invalid("message_required", "ceritakan singkat masalahnya"));
    }
    let context = req.context.unwrap_or(Value::Null);
    if serde_json::to_vec(&context).map(|b| b.len()).unwrap_or(0) > MAX_CONTEXT_BYTES {
        return Err(invalid("context_too_large", "konteks laporan terlalu besar"));
    }

    // Reporting needs the same access as seeing the content: a draft or a
    // locked item can't be reported by someone who can't open it.
    module_item::get_detail(pool, ctx, req.module_item_id).await?;
    let owner = owner_of(pool, req.module_item_id, &req.target_type, &req.content_uid).await?;

    let today = sqlx::query_scalar!(r#"select count(*) as "n!" from content_reports where reporter_id = $1 and created_at > now() - interval '24 hours'"#, ctx.user_id)
        .fetch_one(pool)
        .await?;
    if today >= DAILY_REPORT_LIMIT {
        return Err(AppError::TooManyRequests("report_limit_reached"));
    }

    let mut tx = pool.begin().await?;
    // Join the active ticket for this content, or open one. The partial
    // unique index makes two simultaneous first reports converge.
    let ticket_id = sqlx::query_scalar!(
        r#"insert into content_tickets (module_item_id, target_type, content_uid)
           values ($1, $2, $3)
           on conflict (module_item_id, target_type, content_uid) where status in ('open', 'in_review')
           do update set last_reported_at = now(), updated_at = now()
           returning id"#,
        owner.item_id,
        req.target_type,
        req.content_uid,
    )
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query!(
        r#"insert into content_reports (ticket_id, reporter_id, reporter_role, reported_from_item_id, category, message, context, content_version)
           values ($1, $2, $3, $4, $5, $6, $7, $8)
           on conflict (ticket_id, reporter_id)
           do update set category = excluded.category, message = excluded.message, context = excluded.context, created_at = now()"#,
        ticket_id,
        ctx.user_id,
        ctx.role.as_deref(),
        req.module_item_id,
        req.category,
        message,
        context,
        owner.version,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        r#"update content_tickets t set
             report_count = (select count(*) from content_reports r where r.ticket_id = t.id),
             categories = (select coalesce(array_agg(distinct r.category order by r.category), '{}') from content_reports r where r.ticket_id = t.id),
             last_reported_at = now(), updated_at = now()
           where t.id = $1"#,
        ticket_id,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(CreateReportResponse { received: true })
}

// ── Admin Pusat ─────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct TicketRow {
    pub id: Uuid,
    pub status: String,
    pub target_type: String,
    pub content_uid: String,
    pub module_item_id: Uuid,
    pub report_count: i32,
    pub categories: Vec<String>,
    pub first_reported_at: DateTime<Utc>,
    pub last_reported_at: DateTime<Utc>,
    /// What the ticket is about, short: a question stem or a section title.
    pub summary: String,
    pub context: Option<ItemContext>,
}

#[derive(Debug, Serialize)]
pub struct StatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct TicketListResponse {
    pub items: Vec<TicketRow>,
    pub counts: Vec<StatusCount>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TicketListQuery {
    pub status: Option<String>,
    pub category: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

fn excerpt(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

/// The content as it is NOW (with its key — this is the admin's view).
async fn current_content(pool: &PgPool, item_id: Uuid, target_type: &str, uid: &str) -> Result<Option<Value>, AppError> {
    let Some(row) = sqlx::query!(r#"select quiz_config, lesson_plan, content_type from module_items where id = $1"#, item_id).fetch_optional(pool).await? else {
        return Ok(None);
    };
    Ok(match target_type {
        "question" => find_question(row.quiz_config.as_ref(), uid).map(|(g, q)| serde_json::json!({"group_type": g.get("type"), "instruction": g.get("instruction"), "question": q})),
        "checkpoint_question" => find_checkpoint_question(row.lesson_plan.as_ref(), uid)
            .map(|(s, g, q)| serde_json::json!({"section_id": s.get("id"), "section_title": s.get("title"), "group_type": g.get("type"), "question": q})),
        _ => find_section(row.lesson_plan.as_ref(), uid).map(|s| serde_json::json!({"section": s})),
    })
}

fn summary_of(content: Option<&Value>) -> String {
    let Some(content) = content else { return "(konten sudah tidak ada)".to_string() };
    if let Some(q) = content.get("question") {
        let stem = q.get("stem").or_else(|| q.get("text")).and_then(Value::as_str).unwrap_or("");
        return excerpt(stem, 140);
    }
    content.pointer("/section/title").and_then(Value::as_str).map(|t| excerpt(t, 140)).unwrap_or_default()
}

// GET /admin/content-tickets
pub async fn list(pool: &PgPool, ctx: &AuthContext, query: TicketListQuery) -> Result<TicketListResponse, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::View)?;
    let status = query.status.filter(|s| STATUSES.contains(&s.as_str()));
    let category = query.category.filter(|c| CATEGORIES.contains(&c.as_str()));
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    // Cursor: "<last_reported_at rfc3339>_<id>" — newest activity first.
    let (cursor_at, cursor_id) = match query.cursor.as_deref().and_then(|c| c.rsplit_once('_')) {
        Some((at, id)) => (DateTime::parse_from_rfc3339(at).ok().map(|d| d.with_timezone(&Utc)), id.parse::<Uuid>().ok()),
        None => (None, None),
    };

    let rows = sqlx::query!(
        r#"select id, status, target_type, content_uid, module_item_id, report_count, categories, first_reported_at, last_reported_at
           from content_tickets
           where ($1::text is null or status = $1)
             and ($2::text is null or $2 = any(categories))
             and ($3::timestamptz is null or (last_reported_at, id) < ($3, $4))
           order by last_reported_at desc, id desc
           limit $5"#,
        status,
        category,
        cursor_at,
        cursor_id.unwrap_or(Uuid::max()),
        limit + 1,
    )
    .fetch_all(pool)
    .await?;

    let has_more = rows.len() as i64 > limit;
    let mut items = Vec::new();
    for r in rows.into_iter().take(limit as usize) {
        let content = current_content(pool, r.module_item_id, &r.target_type, &r.content_uid).await?;
        items.push(TicketRow {
            summary: summary_of(content.as_ref()),
            context: content_context::resolve(pool, r.module_item_id).await?,
            id: r.id,
            status: r.status,
            target_type: r.target_type,
            content_uid: r.content_uid,
            module_item_id: r.module_item_id,
            report_count: r.report_count,
            categories: r.categories,
            first_reported_at: r.first_reported_at,
            last_reported_at: r.last_reported_at,
        });
    }
    let next_cursor = if has_more { items.last().map(|t| format!("{}_{}", t.last_reported_at.to_rfc3339(), t.id)) } else { None };

    let counts = sqlx::query!(r#"select status, count(*) as "n!" from content_tickets group by status"#)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| StatusCount { status: r.status, count: r.n })
        .collect();
    Ok(TicketListResponse { items, counts, next_cursor })
}

#[derive(Debug, Serialize)]
pub struct ReportRow {
    pub id: Uuid,
    pub category: String,
    pub message: Option<String>,
    /// Role only — who reported doesn't help fix content, and an admin
    /// page showing learner identities would need its own audit trail.
    pub reporter_role: Option<String>,
    pub reported_from_item_id: Option<Uuid>,
    pub context: Value,
    pub content_version: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct TicketDetailResponse {
    pub ticket: TicketRow,
    pub resolution_note: Option<String>,
    pub handled_by_name: Option<String>,
    pub handled_at: Option<DateTime<Utc>>,
    pub item_content_type: Option<String>,
    pub item_status: Option<String>,
    pub current_version: Option<i32>,
    /// The reported content as it is now, answer key included.
    pub content: Option<Value>,
    pub reports: Vec<ReportRow>,
}

// GET /admin/content-tickets/{id}
pub async fn detail(pool: &PgPool, ctx: &AuthContext, id: Uuid) -> Result<TicketDetailResponse, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::View)?;
    let t = sqlx::query!(
        r#"select t.id, t.status, t.target_type, t.content_uid, t.module_item_id, t.report_count, t.categories, t.first_reported_at, t.last_reported_at,
                  t.resolution_note, t.handled_at, u.name as "handled_by_name?", mi.content_type, mi.status as item_status, mi.current_version
           from content_tickets t
           join module_items mi on mi.id = t.module_item_id
           left join users u on u.id = t.handled_by
           where t.id = $1"#,
        id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("ticket_not_found"))?;

    let content = current_content(pool, t.module_item_id, &t.target_type, &t.content_uid).await?;
    let reports = sqlx::query!(
        r#"select id, category, message, reporter_role, reported_from_item_id, context, content_version, created_at
           from content_reports where ticket_id = $1 order by created_at desc"#,
        id,
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| ReportRow {
        id: r.id,
        category: r.category,
        message: r.message,
        reporter_role: r.reporter_role,
        reported_from_item_id: r.reported_from_item_id,
        context: r.context,
        content_version: r.content_version,
        created_at: r.created_at,
    })
    .collect();

    Ok(TicketDetailResponse {
        ticket: TicketRow {
            summary: summary_of(content.as_ref()),
            context: content_context::resolve(pool, t.module_item_id).await?,
            id: t.id,
            status: t.status,
            target_type: t.target_type,
            content_uid: t.content_uid,
            module_item_id: t.module_item_id,
            report_count: t.report_count,
            categories: t.categories,
            first_reported_at: t.first_reported_at,
            last_reported_at: t.last_reported_at,
        },
        resolution_note: t.resolution_note,
        handled_by_name: t.handled_by_name,
        handled_at: t.handled_at,
        item_content_type: t.content_type,
        item_status: Some(t.item_status),
        current_version: t.current_version,
        content,
        reports,
    })
}

#[derive(Debug, Deserialize)]
pub struct UpdateTicketRequest {
    pub status: String,
    #[serde(default)]
    pub resolution_note: Option<String>,
}

// PATCH /admin/content-tickets/{id}
pub async fn update(pool: &PgPool, ctx: &AuthContext, id: Uuid, req: UpdateTicketRequest) -> Result<TicketDetailResponse, AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::Manage)?;
    if !STATUSES.contains(&req.status.as_str()) {
        return Err(invalid("invalid_status", "status tiket tidak dikenal"));
    }
    let note = req.resolution_note.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    if req.status == "rejected" && note.is_none() {
        return Err(invalid("note_required", "tulis alasan menolak laporan ini"));
    }
    let before = sqlx::query!(r#"select status, resolution_note from content_tickets where id = $1"#, id).fetch_optional(pool).await?.ok_or(AppError::NotFound("ticket_not_found"))?;

    // Re-opening is refused when another active ticket already exists for
    // the same content (the partial unique index would reject it anyway).
    let result = sqlx::query!(
        r#"update content_tickets set status = $2, resolution_note = coalesce($3, resolution_note), handled_by = $4, handled_at = now(), updated_at = now()
           where id = $1"#,
        id,
        req.status,
        note,
        ctx.user_id,
    )
    .execute(pool)
    .await;
    if let Err(sqlx::Error::Database(db)) = &result {
        if db.is_unique_violation() {
            return Err(AppError::Conflict("ticket_already_active"));
        }
    }
    result?;

    admin_audit::record(
        pool,
        Some(ctx.user_id),
        "content_ticket.updated",
        "content_ticket",
        Some(id),
        Some(serde_json::json!({"status": before.status, "resolution_note": before.resolution_note})),
        Some(serde_json::json!({"status": req.status, "resolution_note": note})),
        note.as_deref(),
    )
    .await?;
    detail(pool, ctx, id).await
}
