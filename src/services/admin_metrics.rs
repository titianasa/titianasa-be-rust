// P40-005 (ADR-0014 §2) — the 5 dashboard pages this ticket builds
// (Ringkasan, Penjualan, Kurikulum & Konten, Operasional AI,
// Organisasi). Every page reads from `metrics_daily_*` (P40-004), never
// a live analytic query against a transaction table — the one
// deliberate exception is "today", which P40-004's own hourly job can
// be up to an hour stale on, so a handful of lightweight indexed
// queries here compute "today" directly instead (documented per call).
//
// "Peserta aktif", "Pembelajaran", "Kelas & Guru" stay placeholders
// (ADR-0014 §6 step 5, after Fase 39's event pipeline has enough
// history) — not built here, unchanged from P40-001's sidebar.

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};

fn require_view(ctx: &AuthContext) -> Result<(), AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::View)
}

fn wib_today() -> NaiveDate {
    crate::services::metrics_rollup::wib_today()
}

/// The last 7 WIB days (inclusive of today) — what Penjualan/
/// Operasional AI show when the caller doesn't pass `from`/`to`.
pub fn default_period() -> (NaiveDate, NaiveDate) {
    let today = wib_today();
    (today - chrono::Duration::days(6), today)
}

/// A day range shifted back by its own width — "the same number of
/// days, immediately before `from`" — what every "vs previous period"
/// comparison below measures against. `None` for either bound only
/// when `from`/`to` are already at the epoch (never happens in
/// practice; guards the subtraction from panicking).
fn previous_range(from: NaiveDate, to: NaiveDate) -> (NaiveDate, NaiveDate) {
    let width = (to - from).num_days() + 1;
    (from - chrono::Duration::days(width), from - chrono::Duration::days(1))
}

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub current: i64,
    /// `None` = "belum cukup data" — no source rows exist anywhere in
    /// the previous window (not merely zero activity in it, which is a
    /// real, comparable `Some(0)`).
    pub previous: Option<i64>,
}

async fn earliest_sales_day(pool: &PgPool) -> Result<Option<NaiveDate>, AppError> {
    Ok(sqlx::query_scalar!(r#"select min(day) from metrics_daily_sales"#).fetch_one(pool).await?)
}

async fn earliest_ai_day(pool: &PgPool) -> Result<Option<NaiveDate>, AppError> {
    Ok(sqlx::query_scalar!(r#"select min(day) from metrics_daily_ai"#).fetch_one(pool).await?)
}

async fn earliest_users_day(pool: &PgPool) -> Result<Option<NaiveDate>, AppError> {
    Ok(sqlx::query_scalar!(r#"select min(day) from metrics_daily_users"#).fetch_one(pool).await?)
}

// --- Ringkasan ---

#[derive(Debug, Serialize)]
pub struct Peringatan {
    pub kind: &'static str,
    pub severity: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct CakupanRingkas {
    pub topics_total: i64,
    pub topics_with_module: i64,
    pub topics_with_quiz: i64,
}

#[derive(Debug, Serialize)]
pub struct RingkasanResponse {
    pub pendaftar_baru_7_hari: Comparison,
    pub pendapatan_hari_ini_idr: i64,
    pub pendapatan_bulan_ini_idr: i64,
    pub langganan_aktif: i64,
    pub cakupan_kurikulum: CakupanRingkas,
    pub biaya_ai_7_hari_idr: Comparison,
    pub peringatan: Vec<Peringatan>,
}

pub async fn ringkasan(pool: &PgPool, ctx: &AuthContext) -> Result<RingkasanResponse, AppError> {
    require_view(ctx)?;
    let today = wib_today();

    // "Pendaftar baru 7 hari" — via the aggregate (users don't need
    // today's live freshness at the same urgency as money).
    let (from7, to7) = (today - chrono::Duration::days(6), today);
    let (prev_from7, prev_to7) = previous_range(from7, to7);
    let current_signups: i64 = sqlx::query_scalar!(r#"select coalesce(sum(signups), 0)::bigint as "sum!" from metrics_daily_users where day between $1 and $2"#, from7, to7).fetch_one(pool).await?;
    let previous_signups = match earliest_users_day(pool).await? {
        Some(earliest) if earliest <= prev_from7 => {
            Some(sqlx::query_scalar!(r#"select coalesce(sum(signups), 0)::bigint as "sum!" from metrics_daily_users where day between $1 and $2"#, prev_from7, prev_to7).fetch_one(pool).await?)
        }
        _ => None,
    };

    // "Pendapatan hari ini/bulan ini" — live, indexed on orders(created_at)
    // via the existing btree default on a timestamptz column plus the
    // WHERE clause's range shape; not read from the aggregate, which
    // can lag up to an hour behind on the day money actually matters.
    let pendapatan_hari_ini_idr: i64 = sqlx::query_scalar!(
        r#"select coalesce(sum(amount_idr), 0)::bigint as "sum!" from orders where status = 'paid' and (created_at at time zone 'Asia/Jakarta')::date = $1"#,
        today,
    )
    .fetch_one(pool)
    .await?;

    let month_start = today.with_day(1).expect("day 1 always valid");
    let pendapatan_bulan_ini_idr: i64 = sqlx::query_scalar!(
        r#"select coalesce(sum(amount_idr), 0)::bigint as "sum!" from orders where status = 'paid' and (created_at at time zone 'Asia/Jakarta')::date between $1 and $2"#,
        month_start,
        today,
    )
    .fetch_one(pool)
    .await?;

    let langganan_aktif: i64 = sqlx::query_scalar!(r#"select coalesce(sum(active), 0)::bigint as "sum!" from metrics_daily_subscriptions where day = $1"#, today).fetch_one(pool).await?;

    let cakupan = sqlx::query!(r#"select coalesce(sum(topics_total), 0)::bigint as "topics_total!", coalesce(sum(topics_with_module), 0)::bigint as "topics_with_module!", coalesce(sum(topics_with_quiz), 0)::bigint as "topics_with_quiz!" from metrics_daily_content where day = $1"#, today)
        .fetch_one(pool)
        .await?;

    let from7ai = today - chrono::Duration::days(6);
    let ai_7d = sqlx::query!(r#"select coalesce(sum(cost_idr), 0)::bigint as "cost!", coalesce(sum(calls), 0)::bigint as "calls!", coalesce(sum(failed), 0)::bigint as "failed!" from metrics_daily_ai where day between $1 and $2"#, from7ai, today)
        .fetch_one(pool)
        .await?;
    let (prev_from7ai, prev_to7ai) = previous_range(from7ai, today);
    let previous_ai_cost = match earliest_ai_day(pool).await? {
        Some(earliest) if earliest <= prev_from7ai => {
            Some(sqlx::query_scalar!(r#"select coalesce(sum(cost_idr), 0)::bigint as "sum!" from metrics_daily_ai where day between $1 and $2"#, prev_from7ai, prev_to7ai).fetch_one(pool).await?)
        }
        _ => None,
    };

    let mut peringatan = Vec::new();

    // ADR-0014 §4 — pendapatan turun >40% vs rata-rata 7 hari (before today).
    let avg7_before_today: Option<f64> = sqlx::query_scalar!(
        r#"select avg(daily_revenue)::float8 as "avg" from (
             select day, sum(revenue_idr) as daily_revenue from metrics_daily_sales
             where day between $1 and $2 group by day
           ) t"#,
        today - chrono::Duration::days(7),
        today - chrono::Duration::days(1),
    )
    .fetch_one(pool)
    .await?;
    if let Some(avg) = avg7_before_today {
        if avg > 0.0 && (pendapatan_hari_ini_idr as f64) < avg * 0.6 {
            let pct = 100.0 - (pendapatan_hari_ini_idr as f64 / avg * 100.0);
            peringatan.push(Peringatan {
                kind: "revenue_drop",
                severity: "warning",
                message: format!("Pendapatan hari ini turun {pct:.0}% dibanding rata-rata 7 hari terakhir (Rp{:.0} vs rata-rata Rp{avg:.0})", pendapatan_hari_ini_idr),
            });
        }
    }

    // Tingkat gagal AI >10% (7 hari terakhir).
    if ai_7d.calls > 0 {
        let rate = ai_7d.failed as f64 / ai_7d.calls as f64;
        if rate > 0.10 {
            peringatan.push(Peringatan { kind: "ai_failure_rate", severity: "warning", message: format!("Tingkat gagal panggilan AI {:.0}% dalam 7 hari terakhir ({} dari {})", rate * 100.0, ai_7d.failed, ai_7d.calls) });
        }
    }

    // Antrean job macet >1 jam.
    let stuck_pending: Option<DateTime<Utc>> = sqlx::query_scalar!(r#"select min(run_after) from jobs where status = 'pending' and run_after < now() - interval '1 hour'"#).fetch_one(pool).await?;
    if let Some(oldest) = stuck_pending {
        let age_minutes = (Utc::now() - oldest).num_minutes();
        peringatan.push(Peringatan { kind: "job_queue_stuck", severity: "critical", message: format!("Ada job menunggu lebih dari 1 jam (tertua {age_minutes} menit) — periksa apakah titian-worker berjalan") });
    }

    Ok(RingkasanResponse {
        pendaftar_baru_7_hari: Comparison { current: current_signups, previous: previous_signups },
        pendapatan_hari_ini_idr,
        pendapatan_bulan_ini_idr,
        langganan_aktif,
        cakupan_kurikulum: CakupanRingkas { topics_total: cakupan.topics_total, topics_with_module: cakupan.topics_with_module, topics_with_quiz: cakupan.topics_with_quiz },
        biaya_ai_7_hari_idr: Comparison { current: ai_7d.cost, previous: previous_ai_cost },
        peringatan,
    })
}

// --- Penjualan ---

#[derive(Debug, Serialize)]
pub struct RevenueByKind {
    pub kind: String,
    pub revenue: Comparison,
    pub orders: i64,
    pub paid_orders: i64,
}

#[derive(Debug, Serialize)]
pub struct PenjualanResponse {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub revenue_by_kind: Vec<RevenueByKind>,
    pub mrr_idr: i64,
    pub subscriptions_new: i64,
    pub subscriptions_churned: i64,
    pub marketplace_gmv_idr: i64,
    pub diamond_in: i64,
    pub diamond_out: i64,
    pub ad_views: i64,
}

pub async fn penjualan(pool: &PgPool, ctx: &AuthContext, from: NaiveDate, to: NaiveDate) -> Result<PenjualanResponse, AppError> {
    require_view(ctx)?;
    let (prev_from, prev_to) = previous_range(from, to);
    let earliest = earliest_sales_day(pool).await?;
    let has_previous = matches!(earliest, Some(e) if e <= prev_from);

    let rows = sqlx::query!(
        r#"select kind, sum(revenue_idr)::bigint as "revenue!", sum(orders)::bigint as "orders!", sum(paid_orders)::bigint as "paid_orders!"
           from metrics_daily_sales where day between $1 and $2 group by kind order by kind"#,
        from,
        to,
    )
    .fetch_all(pool)
    .await?;

    let mut revenue_by_kind = Vec::new();
    for row in rows {
        let previous = if has_previous {
            Some(
                sqlx::query_scalar!(r#"select coalesce(sum(revenue_idr), 0)::bigint as "sum!" from metrics_daily_sales where day between $1 and $2 and kind = $3"#, prev_from, prev_to, row.kind)
                    .fetch_one(pool)
                    .await?,
            )
        } else {
            None
        };
        revenue_by_kind.push(RevenueByKind { kind: row.kind, revenue: Comparison { current: row.revenue, previous }, orders: row.orders, paid_orders: row.paid_orders });
    }

    // MRR: active subscriptions on the LATEST day in range × the
    // tier's list price (subscription::tier_price_idr) — the standard
    // "if everyone renews at today's rate" reading of MRR.
    let latest_sub_day: Option<NaiveDate> = sqlx::query_scalar!(r#"select max(day) from metrics_daily_subscriptions where day between $1 and $2"#, from, to).fetch_one(pool).await?;
    let mrr_idr = if let Some(day) = latest_sub_day {
        let active_rows = sqlx::query!(r#"select tier, active from metrics_daily_subscriptions where day = $1"#, day).fetch_all(pool).await?;
        active_rows.iter().map(|r| r.active as i64 * crate::services::subscription::tier_price_idr(&r.tier).unwrap_or(0)).sum()
    } else {
        0
    };

    let sub_totals = sqlx::query!(r#"select coalesce(sum(new), 0)::bigint as "new!", coalesce(sum(churned), 0)::bigint as "churned!" from metrics_daily_subscriptions where day between $1 and $2"#, from, to)
        .fetch_one(pool)
        .await?;

    // "GMV marketplace" — the only marketplace money movement `orders`
    // records is `kind='enrollment'` (tutoring products); paid orders'
    // amount IS the GMV proxy.
    let marketplace_gmv_idr: i64 =
        sqlx::query_scalar!(r#"select coalesce(sum(revenue_idr), 0)::bigint as "sum!" from metrics_daily_sales where day between $1 and $2 and kind = 'enrollment'"#, from, to).fetch_one(pool).await?;

    // Diamond in/out — live from `transactions` (P40-004 doesn't
    // aggregate the credit economy; ADR-0014 §2 lists it only under
    // Penjualan, not worth its own daily table for one page). `amount`
    // is already signed (positive = earn/purchase, negative =
    // spend/expire — see services/economy.rs).
    let diamond = sqlx::query!(
        r#"select coalesce(sum(amount) filter (where amount > 0), 0)::bigint as "in!", coalesce(-sum(amount) filter (where amount < 0), 0)::bigint as "out!"
           from transactions where (created_at at time zone 'Asia/Jakarta')::date between $1 and $2"#,
        from,
        to,
    )
    .fetch_one(pool)
    .await?;

    let ad_views: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from ad_views where (viewed_at at time zone 'Asia/Jakarta')::date between $1 and $2"#, from, to).fetch_one(pool).await?;

    Ok(PenjualanResponse {
        from,
        to,
        revenue_by_kind,
        mrr_idr,
        subscriptions_new: sub_totals.new,
        subscriptions_churned: sub_totals.churned,
        marketplace_gmv_idr,
        diamond_in: diamond.r#in,
        diamond_out: diamond.out,
        ad_views,
    })
}

// --- Kurikulum & Konten ---

#[derive(Debug, Serialize)]
pub struct TahapCoverage {
    pub tahap: i32,
    pub topics_total: i64,
    pub topics_with_module: i64,
    pub topics_with_quiz: i64,
    pub questions_total: i64,
    pub questions_by_bloom: serde_json::Value,
    pub questions_by_bloom_target: serde_json::Value,
    pub questions_by_difficulty: serde_json::Value,
    pub questions_by_difficulty_target: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct SubjectCoverage {
    pub subject_id: Uuid,
    pub subject_name: String,
    pub tahaps: Vec<TahapCoverage>,
}

#[derive(Debug, Serialize)]
pub struct KurikulumResponse {
    pub day: NaiveDate,
    pub subjects: Vec<SubjectCoverage>,
}

/// `metrics_daily_content` has no per-day history to average over (see
/// `metrics_rollup`'s own doc — it's a today-only snapshot), so this
/// page ignores the `from`/`to` period entirely and always shows the
/// LATEST snapshot day, whatever it is.
pub async fn kurikulum(pool: &PgPool, ctx: &AuthContext) -> Result<KurikulumResponse, AppError> {
    require_view(ctx)?;
    let Some(day) = sqlx::query_scalar!(r#"select max(day) from metrics_daily_content"#).fetch_one(pool).await? else {
        return Ok(KurikulumResponse { day: wib_today(), subjects: vec![] });
    };

    let rows = sqlx::query!(
        r#"select c.subject_id as "subject_id!", s.name as "subject_name!", c.tahap, c.topics_total, c.topics_with_module, c.topics_with_quiz,
                  c.questions_total, c.questions_by_bloom, c.questions_by_difficulty
           from metrics_daily_content c
           join subjects s on s.id = c.subject_id
           where c.day = $1
           order by s.name, c.tahap"#,
        day,
    )
    .fetch_all(pool)
    .await?;

    let mut subjects: Vec<SubjectCoverage> = Vec::new();
    for row in rows {
        let (_, bloom_target, difficulty_target) = crate::services::quiz_taxonomy::target_spread(&format!("Tahap {}", row.tahap), row.questions_total.into());
        let bloom_target_json = serde_json::Value::Object(bloom_target.into_iter().map(|(level, n)| (level.id().to_string(), serde_json::json!(n))).collect());
        let difficulty_target_json = serde_json::Value::Object(difficulty_target.into_iter().map(|(level, n)| (level.id().to_string(), serde_json::json!(n))).collect());

        let tahap_coverage = TahapCoverage {
            tahap: row.tahap,
            topics_total: row.topics_total.into(),
            topics_with_module: row.topics_with_module.into(),
            topics_with_quiz: row.topics_with_quiz.into(),
            questions_total: row.questions_total.into(),
            questions_by_bloom: row.questions_by_bloom,
            questions_by_bloom_target: bloom_target_json,
            questions_by_difficulty: row.questions_by_difficulty,
            questions_by_difficulty_target: difficulty_target_json,
        };

        match subjects.iter_mut().find(|s| s.subject_id == row.subject_id) {
            Some(s) => s.tahaps.push(tahap_coverage),
            None => subjects.push(SubjectCoverage { subject_id: row.subject_id, subject_name: row.subject_name, tahaps: vec![tahap_coverage] }),
        }
    }

    Ok(KurikulumResponse { day, subjects })
}

// --- Operasional AI ---

#[derive(Debug, Serialize)]
pub struct AiRoleModelStats {
    pub task_type: String,
    pub model: String,
    pub calls: i64,
    pub failed: i64,
    pub tokens: i64,
    pub cost_idr: i64,
}

#[derive(Debug, Serialize)]
pub struct QueueHealth {
    pub pending: i64,
    pub running: i64,
    pub failed_last_24h: i64,
    pub oldest_pending_age_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct OperasionalAiResponse {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub by_role_model: Vec<AiRoleModelStats>,
    pub queue_health: QueueHealth,
}

pub async fn operasional_ai(pool: &PgPool, ctx: &AuthContext, from: NaiveDate, to: NaiveDate) -> Result<OperasionalAiResponse, AppError> {
    require_view(ctx)?;
    let rows = sqlx::query!(
        r#"select task_type, model, sum(calls)::bigint as "calls!", sum(failed)::bigint as "failed!", sum(tokens)::bigint as "tokens!", sum(cost_idr)::bigint as "cost_idr!"
           from metrics_daily_ai where day between $1 and $2 group by task_type, model order by sum(calls) desc"#,
        from,
        to,
    )
    .fetch_all(pool)
    .await?;
    let by_role_model = rows.into_iter().map(|r| AiRoleModelStats { task_type: r.task_type, model: r.model, calls: r.calls, failed: r.failed, tokens: r.tokens, cost_idr: r.cost_idr }).collect();

    let queue = sqlx::query!(r#"select count(*) filter (where status = 'pending') as "pending!", count(*) filter (where status = 'running') as "running!" from jobs"#).fetch_one(pool).await?;
    let failed_24h: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from jobs where status = 'failed' and finished_at > now() - interval '24 hours'"#).fetch_one(pool).await?;
    let oldest_pending: Option<DateTime<Utc>> = sqlx::query_scalar!(r#"select min(run_after) from jobs where status = 'pending'"#).fetch_one(pool).await?;
    let oldest_pending_age_seconds = oldest_pending.map(|t| (Utc::now() - t).num_seconds().max(0));

    Ok(OperasionalAiResponse {
        from,
        to,
        by_role_model,
        queue_health: QueueHealth { pending: queue.pending, running: queue.running, failed_last_24h: failed_24h, oldest_pending_age_seconds },
    })
}

// --- Organisasi ---

#[derive(Debug, Serialize)]
pub struct OrganizationSummary {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub r#type: String,
    pub member_count: i64,
    pub last_activity: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct OrganisasiResponse {
    pub organizations: Vec<OrganizationSummary>,
}

/// Live (not aggregate-backed — ADR-0014 §2 lists no daily table for
/// this page, and an organization list is small/cheap to read directly
/// with an indexed GROUP BY). "Aktivitas" = the most recent membership
/// added to that org — the only org-scoped timestamp signal that
/// exists today without a dedicated activity log.
pub async fn organisasi(pool: &PgPool, ctx: &AuthContext) -> Result<OrganisasiResponse, AppError> {
    require_view(ctx)?;
    let rows = sqlx::query!(
        r#"select o.id, o.name, o.slug, o.type, count(uor.id)::bigint as "member_count!", max(uor.created_at) as last_activity
           from organizations o
           left join user_organization_roles uor on uor.organization_id = o.id
           where o.type != 'platform'
           group by o.id, o.name, o.slug, o.type
           order by o.name"#
    )
    .fetch_all(pool)
    .await?;

    Ok(OrganisasiResponse {
        organizations: rows.into_iter().map(|r| OrganizationSummary { id: r.id, name: r.name, slug: r.slug, r#type: r.r#type, member_count: r.member_count, last_activity: r.last_activity }).collect(),
    })
}
