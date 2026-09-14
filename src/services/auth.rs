use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::responses::auth::{LoginResponse, RefreshResponse, RoleEntry, UserSummary};
use crate::services::request_meta::RequestMeta;
use crate::services::{google_oauth::GoogleTokenVerifier, token};
use crate::Config;

const PLATFORM_ORG_SLUG: &str = "titian-asa";

pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub name: String,
}

// Port of user_repository.ts's findByGoogleId/findByEmail/findById.
async fn find_user_by_google_id(pool: &PgPool, google_id: &str) -> Result<Option<UserRow>, AppError> {
    let row = sqlx::query_as!(
        UserRow,
        r#"select id, email, name from users where google_id = $1"#,
        google_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn find_user_by_email(pool: &PgPool, email: &str) -> Result<Option<UserRow>, AppError> {
    let row = sqlx::query_as!(UserRow, r#"select id, email, name from users where email = $1"#, email)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn find_user_by_id(pool: &PgPool, id: Uuid) -> Result<Option<UserRow>, AppError> {
    let row = sqlx::query_as!(UserRow, r#"select id, email, name from users where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

// Port of user_repository.ts's insert/setGoogleId.
async fn insert_user(
    pool: &PgPool,
    google_id: &str,
    email: &str,
    name: &str,
    avatar_url: Option<&str>,
) -> Result<UserRow, AppError> {
    let row = sqlx::query_as!(
        UserRow,
        r#"insert into users (google_id, email, name, avatar_url) values ($1, $2, $3, $4)
           returning id, email, name"#,
        google_id,
        email,
        name,
        avatar_url,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn set_google_id(pool: &PgPool, user_id: Uuid, google_id: &str) -> Result<UserRow, AppError> {
    let row = sqlx::query_as!(
        UserRow,
        r#"update users set google_id = $2 where id = $1 returning id, email, name"#,
        user_id,
        google_id,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

// Port of organization_repository.ts's findOrCreatePlatformOrg —
// idempotent via onConflictDoNothing there; here via
// `ON CONFLICT ... DO NOTHING` + a fallback select, same two-step shape.
async fn find_or_create_platform_org(pool: &PgPool) -> Result<Uuid, AppError> {
    let inserted = sqlx::query_scalar!(
        r#"insert into organizations (name, slug, type) values ('Titian Asa', $1, 'platform')
           on conflict (slug) do nothing
           returning id"#,
        PLATFORM_ORG_SLUG,
    )
    .fetch_optional(pool)
    .await?;
    if let Some(id) = inserted {
        return Ok(id);
    }
    let existing = sqlx::query_scalar!(
        r#"select id from organizations where slug = $1"#,
        PLATFORM_ORG_SLUG,
    )
    .fetch_one(pool)
    .await?;
    Ok(existing)
}

// Port of organization_repository.ts's assignDefaultStudentRole.
async fn assign_default_student_role(pool: &PgPool, user_id: Uuid) -> Result<(), AppError> {
    let org_id = find_or_create_platform_org(pool).await?;
    sqlx::query!(
        r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, 'student')
           on conflict (user_id, organization_id, role) do nothing"#,
        user_id,
        org_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub struct RoleRow {
    pub organization_id: Uuid,
    pub role: String,
}

// Port of user_repository.ts's findAllRoles. Ordered (not just a plain
// select) because `account-sync.tsx`'s own doc comment treats `roles[0]`
// as "the account's default org" everywhere in the frontend — there's no
// org-switcher UI. Admin-tier roles first, most-recent first, so a
// freshly created/promoted-into org naturally becomes "the" active one
// without any other frontend change (Phase 33).
pub async fn find_all_roles(pool: &PgPool, user_id: Uuid) -> Result<Vec<RoleRow>, AppError> {
    let rows = sqlx::query_as!(
        RoleRow,
        r#"select organization_id, role from user_organization_roles where user_id = $1
           order by case when role in ('platform_admin', 'org_owner', 'academic_director') then 0 else 1 end,
                    created_at desc"#,
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// Port of user_repository.ts's findRoleInOrg/findDefaultRole — used by
// middleware/auth.rs to resolve the "active" org.
pub async fn find_role_in_org(pool: &PgPool, user_id: Uuid, organization_id: Uuid) -> Result<Option<RoleRow>, AppError> {
    let row = sqlx::query_as!(
        RoleRow,
        r#"select organization_id, role from user_organization_roles
           where user_id = $1 and organization_id = $2 limit 1"#,
        user_id,
        organization_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// Phase 33 — must stay in lockstep with find_all_roles' ordering
// (same case/created_at expression). This is the org `require_permission_
// in_org` actually gates against per-request (middleware/auth.rs's
// resolve_auth_context, when no `x-organization-id` header is sent) —
// before this fix it independently used `created_at asc` (oldest role),
// while find_all_roles (what `/users/me` returns, what the frontend
// treats as "the active org") used the new priority order. That
// mismatch meant a user could see their new org as "active" in the UI
// while every permission check still silently gated against their OLD
// org, 403ing on their own freshly created organization.
pub async fn find_default_role(pool: &PgPool, user_id: Uuid) -> Result<Option<RoleRow>, AppError> {
    let row = sqlx::query_as!(
        RoleRow,
        r#"select organization_id, role from user_organization_roles where user_id = $1
           order by case when role in ('platform_admin', 'org_owner', 'academic_director') then 0 else 1 end,
                    created_at desc
           limit 1"#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn insert_refresh_token(
    pool: &PgPool,
    user_id: Uuid,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into refresh_tokens (user_id, token_hash, expires_at) values ($1, $2, $3)"#,
        user_id,
        token_hash,
        expires_at,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Port of auth_repository.ts's findValidByHash.
async fn find_valid_refresh_token_user(pool: &PgPool, token_hash: &str) -> Result<Option<Uuid>, AppError> {
    let user_id = sqlx::query_scalar!(
        r#"select user_id from refresh_tokens
           where token_hash = $1 and revoked_at is null and expires_at > now()
           limit 1"#,
        token_hash,
    )
    .fetch_optional(pool)
    .await?;
    Ok(user_id)
}

async fn issue_tokens(pool: &PgPool, config: &Config, user: &UserRow, meta: &RequestMeta) -> Result<LoginResponse, AppError> {
    let access_token = token::issue_access_token(
        &config.jwt_access_secret,
        &user.id.to_string(),
        &user.email,
        config.access_token_ttl_minutes,
    )
    .map_err(AppError::Internal)?;

    let (raw_refresh, hash) = token::generate_refresh_token();
    let expires_at = Utc::now() + chrono::Duration::days(config.refresh_token_ttl_days);
    insert_refresh_token(pool, user.id, &hash, expires_at).await?;

    // Admin Pusat "Peserta" — login history + an immediate presence
    // touch, so "online sekarang" doesn't wait for the first
    // authenticated request after this one.
    if let Err(err) = record_login_event(pool, user.id, meta).await {
        tracing::warn!(?err, user_id = %user.id, "gagal mencatat login event");
    }
    if let Err(err) = touch_presence(pool, user.id, meta).await {
        tracing::warn!(?err, user_id = %user.id, "gagal mencatat presence saat login");
    }

    Ok(LoginResponse {
        access_token,
        refresh_token: raw_refresh,
        user: UserSummary { id: user.id, email: user.email.clone(), name: user.name.clone() },
    })
}

async fn record_login_event(pool: &PgPool, user_id: Uuid, meta: &RequestMeta) -> Result<(), AppError> {
    let ip_text = meta.ip.map(|ip| ip.to_string());
    sqlx::query!(
        r#"insert into user_login_events (user_id, ip, user_agent, device_label) values ($1, $2, $3, $4)"#,
        user_id,
        ip_text,
        meta.user_agent,
        meta.device_label,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Admin Pusat "Peserta" — cheap enough to call on every authenticated
// request (middleware/auth.rs) because the UPDATE's own WHERE clause
// skips the write entirely when the row is still fresh (<2 minutes),
// so in practice this only actually touches disk a couple of times a
// minute per active user, not once a request.
pub async fn touch_presence(pool: &PgPool, user_id: Uuid, meta: &RequestMeta) -> Result<(), AppError> {
    let ip_text = meta.ip.map(|ip| ip.to_string());
    sqlx::query!(
        r#"insert into user_presence (user_id, last_seen_at, last_ip, last_user_agent, last_device_label)
           values ($1, now(), $2, $3, $4)
           on conflict (user_id) do update
             set last_seen_at = excluded.last_seen_at,
                 last_ip = excluded.last_ip,
                 last_user_agent = excluded.last_user_agent,
                 last_device_label = excluded.last_device_label
             where user_presence.last_seen_at < now() - interval '2 minutes'"#,
        user_id,
        ip_text,
        meta.user_agent,
        meta.device_label,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// Port of auth_service.ts's googleCallback.
pub async fn google_callback(
    pool: &PgPool,
    verifier: &GoogleTokenVerifier,
    config: &Config,
    id_token: &str,
    meta: &RequestMeta,
) -> Result<LoginResponse, AppError> {
    let claims = verifier.verify(id_token, &config.google_client_id).await?;

    let user = match find_user_by_google_id(pool, &claims.sub).await? {
        Some(u) => u,
        None => match find_user_by_email(pool, &claims.email).await? {
            Some(u) => set_google_id(pool, u.id, &claims.sub).await?,
            None => {
                let name = claims.name.clone().unwrap_or_else(|| claims.email.clone());
                insert_user(pool, &claims.sub, &claims.email, &name, claims.picture.as_deref()).await?
            }
        },
    };

    // P9-002 auto-heal, ported as-is: a user with zero org roles gets a
    // permanently-null AuthContext, so this is checked on every login,
    // not just at insert.
    let roles = find_all_roles(pool, user.id).await?;
    if roles.is_empty() {
        assign_default_student_role(pool, user.id).await?;
    }

    issue_tokens(pool, config, &user, meta).await
}

// Port of auth_service.ts's refresh.
pub async fn refresh(pool: &PgPool, config: &Config, raw_refresh_token: &str) -> Result<RefreshResponse, AppError> {
    let hash = token::hash_refresh_token(raw_refresh_token);
    let user_id = find_valid_refresh_token_user(pool, &hash)
        .await?
        .ok_or(AppError::InvalidRefreshToken)?;
    let user = find_user_by_id(pool, user_id).await?.ok_or(AppError::InvalidRefreshToken)?;

    let access_token = token::issue_access_token(
        &config.jwt_access_secret,
        &user.id.to_string(),
        &user.email,
        config.access_token_ttl_minutes,
    )
    .map_err(AppError::Internal)?;

    Ok(RefreshResponse { access_token })
}

// Port of user_handler.ts's getMe.
pub async fn get_me(pool: &PgPool, user_id: Uuid) -> Result<(UserRow, Vec<RoleEntry>), AppError> {
    let user = find_user_by_id(pool, user_id).await?.ok_or(AppError::Unauthorized)?;
    let roles = find_all_roles(pool, user_id).await?;
    let roles = roles
        .into_iter()
        .map(|r| RoleEntry { organization_id: r.organization_id, role: r.role })
        .collect();
    Ok((user, roles))
}
