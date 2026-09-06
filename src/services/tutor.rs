use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::responses::tutor::{TutorListRowResponse, TutorProfileResponse};
use crate::services::permissions::{require_permission_in_org, Action, Resource};

struct TutorProfileRow {
    user_id: Uuid,
    organization_id: Uuid,
    bio: String,
    specializations: serde_json::Value,
}

impl From<TutorProfileRow> for TutorProfileResponse {
    fn from(r: TutorProfileRow) -> Self {
        Self { user_id: r.user_id, organization_id: r.organization_id, bio: r.bio, specializations: r.specializations }
    }
}

// Port of tutor_repository.ts's assign — assigns the `tutor` role AND
// creates the profile row in 1 transaction, idempotent on both (a
// caller re-assigning an existing tutor gets the existing row back,
// not an error).
async fn assign(
    pool: &PgPool,
    user_id: Uuid,
    organization_id: Uuid,
    bio: &str,
    specializations: &serde_json::Value,
) -> Result<TutorProfileRow, AppError> {
    let mut tx = pool.begin().await?;

    sqlx::query!(
        r#"insert into user_organization_roles (user_id, organization_id, role) values ($1, $2, 'tutor')
           on conflict (user_id, organization_id, role) do nothing"#,
        user_id,
        organization_id,
    )
    .execute(&mut *tx)
    .await?;

    let inserted = sqlx::query_as!(
        TutorProfileRow,
        r#"insert into tutor_profiles (user_id, organization_id, bio, specializations)
           values ($1, $2, $3, $4)
           on conflict (user_id) do nothing
           returning user_id, organization_id, bio, specializations"#,
        user_id,
        organization_id,
        bio,
        specializations,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let row = match inserted {
        Some(row) => row,
        None => sqlx::query_as!(
            TutorProfileRow,
            r#"select user_id, organization_id, bio, specializations from tutor_profiles where user_id = $1"#,
            user_id,
        )
        .fetch_one(&mut *tx)
        .await?,
    };

    tx.commit().await?;
    Ok(row)
}

// Exposed for learning_product.rs's lazy tutor-profile provisioning —
// just needs to know whether a row exists, not its contents.
pub async fn find_profile_row(pool: &PgPool, user_id: Uuid) -> Result<Option<()>, AppError> {
    Ok(find_profile(pool, user_id).await?.map(|_| ()))
}

// Exposed for learning_product.rs — same assign() this module's own
// assign_tutor uses, with empty bio/specializations (matching Bun's
// `tutorRepository.assign(db, ctx.userId, ctx.organizationId, "", [])`
// lazy-provisioning call).
pub async fn assign_tutor_role(pool: &PgPool, user_id: Uuid, organization_id: Uuid) -> Result<(), AppError> {
    assign(pool, user_id, organization_id, "", &serde_json::json!([])).await?;
    Ok(())
}

async fn find_profile(pool: &PgPool, user_id: Uuid) -> Result<Option<TutorProfileRow>, AppError> {
    let row = sqlx::query_as!(
        TutorProfileRow,
        r#"select user_id, organization_id, bio, specializations from tutor_profiles where user_id = $1"#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

async fn update_profile(
    pool: &PgPool,
    user_id: Uuid,
    bio: &str,
    specializations: &serde_json::Value,
) -> Result<TutorProfileRow, AppError> {
    let row = sqlx::query_as!(
        TutorProfileRow,
        r#"update tutor_profiles set bio = $2, specializations = $3 where user_id = $1
           returning user_id, organization_id, bio, specializations"#,
        user_id,
        bio,
        specializations,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn list_by_organization(pool: &PgPool, organization_id: Uuid) -> Result<Vec<TutorListRowResponse>, AppError> {
    let rows = sqlx::query!(
        r#"select t.user_id as "user_id!", u.name as "name!", t.bio as "bio!", t.specializations as "specializations!"
           from tutor_profiles t
           inner join users u on u.id = t.user_id
           where t.organization_id = $1"#,
        organization_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TutorListRowResponse { user_id: r.user_id, name: r.name, bio: r.bio, specializations: r.specializations })
        .collect())
}

// POST /organizations/{id}/tutors
pub async fn assign_tutor(
    pool: &PgPool,
    ctx: &AuthContext,
    organization_id: Uuid,
    target_user_id: Uuid,
    bio: Option<String>,
    specializations: Option<Vec<String>>,
) -> Result<TutorProfileResponse, AppError> {
    require_permission_in_org(ctx, organization_id, Resource::TutorProfile, Action::Create)?;
    let bio = bio.unwrap_or_default();
    let specializations = serde_json::to_value(specializations.unwrap_or_default()).unwrap();
    let row = assign(pool, target_user_id, organization_id, &bio, &specializations).await?;
    Ok(row.into())
}

// GET /organizations/{id}/tutors — deliberately looser than
// organization_members:view: any authenticated user can browse an
// org's tutor listing, same as browsing a marketplace.
pub async fn list_tutors(pool: &PgPool, organization_id: Uuid) -> Result<Vec<TutorListRowResponse>, AppError> {
    list_by_organization(pool, organization_id).await
}

// PATCH /tutors/me — ownership check, not a role-matrix question: the
// target is always the caller. Undefined fields mean "leave unchanged"
// (true PATCH semantics), falling back to the existing row's value.
pub async fn update_own_profile(
    pool: &PgPool,
    ctx: &AuthContext,
    bio: Option<String>,
    specializations: Option<Vec<String>>,
) -> Result<TutorProfileResponse, AppError> {
    let existing = find_profile(pool, ctx.user_id).await?.ok_or(AppError::NotFound("tutor_profile_not_found"))?;
    let bio = bio.unwrap_or(existing.bio);
    let specializations = match specializations {
        Some(s) => serde_json::to_value(s).unwrap(),
        None => existing.specializations,
    };
    let row = update_profile(pool, ctx.user_id, &bio, &specializations).await?;
    Ok(row.into())
}
