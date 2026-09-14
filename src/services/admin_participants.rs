// Admin Pusat "Peserta" (ADR-0014 §2) plus the two pieces the ADR never
// speced: "sedang online sekarang" and a per-participant detail view
// (IP/device/region/login history/activity). The aggregate page reads
// `metrics_daily_participants`/`metrics_weekly_retention` (kept fresh by
// the `metrics_rollup` job, services/metrics_rollup.rs) exactly like the
// other 5 P40-005 pages read their own `metrics_daily_*` tables — the
// one live exception is "sedang online", same "today can't wait for an
// hourly job" reasoning Ringkasan already uses for money.
//
// Every individual data view is audit-logged (ADR-0014 §5: "setiap
// pembukaan data individu dicatat di log audit") — `participant_detail`
// calls `admin_audit::record` unconditionally, not just on some paths.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::admin_audit;
use crate::services::admin_metrics::Comparison;
use crate::services::permissions::{require_permission, Action, Resource};

fn require_view(ctx: &AuthContext) -> Result<(), AppError> {
    require_permission(ctx, Resource::AdminPusat, Action::View)
}

// --- Peserta (aggregate) ---

#[derive(Debug, Serialize)]
pub struct RetentionRow {
    pub cohort_week: NaiveDate,
    pub weeks_since_signup: i32,
    pub cohort_size: i32,
    pub active_users: i32,
}

#[derive(Debug, Serialize)]
pub struct DistribusiItem {
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct Corong {
    pub daftar: i64,
    pub aktivitas_pertama: i64,
    pub aktif_minggu_2: i64,
}

#[derive(Debug, Serialize)]
pub struct PesertaResponse {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub dau: Comparison,
    pub wau: Comparison,
    pub mau: Comparison,
    pub sedang_online: i64,
    pub retensi: Vec<RetentionRow>,
    pub sebaran_jenjang: Vec<DistribusiItem>,
    pub sebaran_organisasi: Vec<DistribusiItem>,
    pub corong: Corong,
}

/// `metrics_daily_participants` always has a contiguous row for every
/// WIB day from the earliest signup onward (`rollup_day` writes it
/// every run, backfilled once historically) — so a MISSING row means
/// "before this feature had data" (→ `None`, "belum cukup data"), and a
/// PRESENT row's dau/wau/mau of 0 is a real, comparable zero. No
/// separate earliest-day check needed, unlike the sum-over-range
/// comparisons elsewhere in admin_metrics.rs.
pub async fn peserta(pool: &PgPool, ctx: &AuthContext, from: NaiveDate, to: NaiveDate) -> Result<PesertaResponse, AppError> {
    require_view(ctx)?;

    let current = sqlx::query!(r#"select dau, wau, mau from metrics_daily_participants where day = $1"#, to).fetch_optional(pool).await?;
    let (dau_now, wau_now, mau_now) = current.map(|r| (r.dau as i64, r.wau as i64, r.mau as i64)).unwrap_or((0, 0, 0));

    // Same "shift back by the period's own width" idea as
    // admin_metrics::previous_range, but only the END of that previous
    // window matters here (a snapshot metric, not a sum over a range).
    let prev_to = from - chrono::Duration::days(1);
    let previous = sqlx::query!(r#"select dau, wau, mau from metrics_daily_participants where day = $1"#, prev_to).fetch_optional(pool).await?;
    let (dau_prev, wau_prev, mau_prev) = match previous {
        Some(r) => (Some(r.dau as i64), Some(r.wau as i64), Some(r.mau as i64)),
        None => (None, None, None),
    };

    // "Sedang online" — live, not from the aggregate: presence changes
    // by the minute, an hourly job would make this lie constantly.
    let sedang_online: i64 = sqlx::query_scalar!(r#"select count(*) as "count!" from user_presence where last_seen_at > now() - interval '5 minutes'"#).fetch_one(pool).await?;

    let retensi_rows = sqlx::query!(r#"select cohort_week, weeks_since_signup, cohort_size, active_users from metrics_weekly_retention order by cohort_week desc, weeks_since_signup asc"#)
        .fetch_all(pool)
        .await?;
    let retensi = retensi_rows.into_iter().map(|r| RetentionRow { cohort_week: r.cohort_week, weeks_since_signup: r.weeks_since_signup, cohort_size: r.cohort_size, active_users: r.active_users }).collect();

    // All-time distribution (not period-scoped) — "who our users are",
    // not "who signed up this week"; matches how Organisasi's page reads
    // live and un-paginated for the same "small, always current" reason.
    let jenjang_rows = sqlx::query!(
        r#"select coalesce(jenjang, 'Belum diisi') as "jenjang!", count(*)::bigint as "count!"
           from users u left join user_learning_profiles p on p.user_id = u.id
           group by coalesce(jenjang, 'Belum diisi') order by count(*) desc"#
    )
    .fetch_all(pool)
    .await?;
    let sebaran_jenjang = jenjang_rows.into_iter().map(|r| DistribusiItem { label: r.jenjang, count: r.count }).collect();

    let org_rows = sqlx::query!(
        r#"select o.name as "name!", count(*)::bigint as "count!"
           from user_organization_roles uor join organizations o on o.id = uor.organization_id
           where o.type != 'platform'
           group by o.name order by count(*) desc limit 10"#
    )
    .fetch_all(pool)
    .await?;
    let sebaran_organisasi = org_rows.into_iter().map(|r| DistribusiItem { label: r.name, count: r.count }).collect();

    // Corong sederhana untuk kohort yang mendaftar di periode ini: daftar
    // -> pernah beraktivitas -> masih aktif di minggu ke-2. Live, dibatasi
    // ke kohort periode terpilih (bukan seluruh learning_events).
    let corong = sqlx::query!(
        r#"with cohort as (
             select id, created_at from users where (created_at at time zone 'Asia/Jakarta')::date between $1 and $2
           )
           select
             (select count(*) from cohort) as "daftar!",
             (select count(distinct c.id) from cohort c join learning_events le on le.user_id = c.id) as "aktivitas_pertama!",
             (select count(distinct c.id) from cohort c join learning_events le on le.user_id = c.id
                and le.created_at >= c.created_at + interval '7 days' and le.created_at < c.created_at + interval '14 days') as "aktif_minggu_2!""#,
        from,
        to,
    )
    .fetch_one(pool)
    .await?;

    Ok(PesertaResponse {
        from,
        to,
        dau: Comparison { current: dau_now, previous: dau_prev },
        wau: Comparison { current: wau_now, previous: wau_prev },
        mau: Comparison { current: mau_now, previous: mau_prev },
        sedang_online,
        retensi,
        sebaran_jenjang,
        sebaran_organisasi,
        corong: Corong { daftar: corong.daftar, aktivitas_pertama: corong.aktivitas_pertama, aktif_minggu_2: corong.aktif_minggu_2 },
    })
}

// --- Pencarian & detail pengguna ---

#[derive(Debug, Serialize)]
pub struct ParticipantSummary {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub avatar_url: Option<String>,
    pub is_online: bool,
    pub last_seen_at: Option<DateTime<Utc>>,
}

fn is_online(last_seen_at: Option<DateTime<Utc>>) -> bool {
    last_seen_at.map(|t| Utc::now() - t < chrono::Duration::minutes(5)).unwrap_or(false)
}

pub async fn search_participants(pool: &PgPool, ctx: &AuthContext, q: &str, limit: i64) -> Result<Vec<ParticipantSummary>, AppError> {
    require_view(ctx)?;
    let pattern = format!("%{}%", q.trim());
    let rows = sqlx::query!(
        r#"select u.id, u.name, u.email, u.avatar_url, p.last_seen_at as "last_seen_at?"
           from users u left join user_presence p on p.user_id = u.id
           where u.name ilike $1 or u.email ilike $1
           order by u.name
           limit $2"#,
        pattern,
        limit.clamp(1, 50),
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ParticipantSummary { id: r.id, name: r.name, email: r.email, avatar_url: r.avatar_url, is_online: is_online(r.last_seen_at), last_seen_at: r.last_seen_at })
        .collect())
}

#[derive(Debug, Serialize)]
pub struct LoginEventRow {
    pub occurred_at: DateTime<Utc>,
    pub ip: Option<String>,
    pub device_label: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ActivityRow {
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub source: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ParticipantDetailResponse {
    pub id: Uuid,
    pub name: String,
    pub email: String,
    pub avatar_url: Option<String>,
    pub jenjang: Option<String>,
    pub goal: Option<String>,
    pub created_at: DateTime<Utc>,
    pub is_online: bool,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub last_ip: Option<String>,
    pub last_device_label: Option<String>,
    pub region: Option<String>,
    pub login_history: Vec<LoginEventRow>,
    pub activity: Vec<ActivityRow>,
}

pub async fn participant_detail(pool: &PgPool, ctx: &AuthContext, user_id: Uuid) -> Result<ParticipantDetailResponse, AppError> {
    require_view(ctx)?;

    let profile = sqlx::query!(
        r#"select u.id, u.name, u.email, u.avatar_url, u.created_at, p.jenjang, p.goal
           from users u left join user_learning_profiles p on p.user_id = u.id
           where u.id = $1"#,
        user_id,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("user_not_found"))?;

    let presence = sqlx::query!(r#"select last_seen_at, last_ip, last_device_label, region, region_resolved_at from user_presence where user_id = $1"#, user_id)
        .fetch_optional(pool)
        .await?;

    let region = resolve_region(pool, user_id, presence.as_ref().and_then(|p| p.last_ip.clone()), presence.as_ref().and_then(|p| p.region.clone()), presence.as_ref().and_then(|p| p.region_resolved_at))
        .await
        .unwrap_or_else(|err| {
            tracing::warn!(?err, %user_id, "gagal resolve region, dibiarkan kosong");
            presence.as_ref().and_then(|p| p.region.clone())
        });

    let login_rows = sqlx::query!(r#"select occurred_at, ip, device_label from user_login_events where user_id = $1 order by occurred_at desc limit 20"#, user_id)
        .fetch_all(pool)
        .await?;

    // occurred_at (not created_at) — when the learner actually did this,
    // the honest thing to show an admin reading their activity back.
    let activity_rows = sqlx::query!(r#"select occurred_at, event_type, source, payload from learning_events where user_id = $1 order by occurred_at desc limit 50"#, user_id)
        .fetch_all(pool)
        .await?;

    admin_audit::record(pool, Some(ctx.user_id), "participant.viewed", "user", Some(user_id), None, None, None).await?;

    Ok(ParticipantDetailResponse {
        id: profile.id,
        name: profile.name,
        email: profile.email,
        avatar_url: profile.avatar_url,
        jenjang: profile.jenjang,
        goal: profile.goal,
        created_at: profile.created_at,
        is_online: is_online(presence.as_ref().map(|p| p.last_seen_at)),
        last_seen_at: presence.as_ref().map(|p| p.last_seen_at),
        last_ip: presence.as_ref().and_then(|p| p.last_ip.clone()),
        last_device_label: presence.as_ref().and_then(|p| p.last_device_label.clone()),
        region,
        login_history: login_rows.into_iter().map(|r| LoginEventRow { occurred_at: r.occurred_at, ip: r.ip, device_label: r.device_label }).collect(),
        activity: activity_rows.into_iter().map(|r| ActivityRow { occurred_at: r.occurred_at, event_type: r.event_type, source: r.source, payload: r.payload }).collect(),
    })
}

/// Region is resolved LAZILY, only from this detail page — never on the
/// login/request hot path. Cached ≤24h in `user_presence.region` so
/// repeat views of the same participant don't re-call the third-party
/// API. Best-effort: any failure (no IP yet, private/loopback dev IP,
/// the API being down) just leaves region blank, never breaks the page.
async fn resolve_region(pool: &PgPool, user_id: Uuid, ip: Option<String>, cached_region: Option<String>, cached_at: Option<DateTime<Utc>>) -> Result<Option<String>, anyhow::Error> {
    let fresh = cached_region.is_some() && cached_at.map(|t| Utc::now() - t < chrono::Duration::hours(24)).unwrap_or(false);
    if fresh {
        return Ok(cached_region);
    }
    let Some(ip) = ip else { return Ok(None) };
    let Some(region) = lookup_region(&ip).await? else { return Ok(None) };

    sqlx::query!(r#"update user_presence set region = $2, region_resolved_at = now() where user_id = $1"#, user_id, region)
        .execute(pool)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Some(region))
}

fn is_private_or_loopback(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct IpApiResponse {
    status: String,
    #[serde(rename = "regionName")]
    region_name: Option<String>,
    city: Option<String>,
}

async fn lookup_region(ip: &str) -> Result<Option<String>, anyhow::Error> {
    let Ok(parsed) = ip.parse::<std::net::IpAddr>() else { return Ok(None) };
    if is_private_or_loopback(&parsed) {
        return Ok(None);
    }

    let client = reqwest::Client::new();
    let resp: IpApiResponse = client
        .get(format!("http://ip-api.com/json/{ip}?lang=id&fields=status,regionName,city"))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await?
        .json()
        .await?;

    if resp.status != "success" {
        return Ok(None);
    }
    Ok(match (resp.city, resp.region_name) {
        (Some(city), Some(region)) if !city.is_empty() && !region.is_empty() => Some(format!("{city}, {region}")),
        (Some(city), _) if !city.is_empty() => Some(city),
        (_, Some(region)) if !region.is_empty() => Some(region),
        _ => None,
    })
}
