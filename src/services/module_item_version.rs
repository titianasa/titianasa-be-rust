// P39-002 (ADR-0013 L1) — a permanent snapshot of an item's content
// taken at every publish, alongside `module_items`' own mutable current
// state. Extends ADR-0008's versioning idea (which only ever covered
// the older `questions`/`lessons` bank) to the `lesson_plan`/
// `quiz_config` content model Phase 37/38 authoring actually writes.
//
// Why this exists: ADR-0013's content optimizer needs to say "v1.1
// scored better than v1.0" — impossible without a frozen v1.0 to
// compare against once the author edits the item again. It's also what
// lets a learner's attempt keep pointing at the EXACT content it was
// scored against (`attempts.content_version`), the same fairness
// guarantee ADR-0008's `question_snapshot` already gives the old bank.
//
// `freeze` is deliberately permission-agnostic — the caller (today,
// `module_item::publish`) has already checked who may publish; a future
// caller (Phase 42's content-agent proposals) will have its own,
// different authorization story, and duplicating a check here would
// only risk the two drifting apart.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::models::auth::AuthContext;

/// sha256 hex of the frozen content — cheap to compare so a caller can
/// tell two versions apart without diffing potentially-large jsonb.
/// `lesson_plan` and `quiz_config` are hashed as their canonical
/// `to_string()` form; a field absent (`None`) hashes as an empty
/// string, so "no content of this kind" is stable and distinct from any
/// real content (which is always non-empty JSON).
fn content_hash(lesson_plan: Option<&serde_json::Value>, quiz_config: Option<&serde_json::Value>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(lesson_plan.map(|v| v.to_string()).unwrap_or_default());
    // A separator between the two fields' JSON, so {"a":1} + "" can
    // never collide with "" + {"a":1} the way plain concatenation would.
    hasher.update(b"\0");
    hasher.update(quiz_config.map(|v| v.to_string()).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}

/// Freezes the given content as the next version of `item_id`: supersedes
/// whichever version was current (if any), inserts the new row, and
/// advances `module_items.current_version`. Returns the new version
/// number. A no-op-looking call (both `lesson_plan` and `quiz_config`
/// are `None`) still records a version — the caller (not this function)
/// decides whether freezing "nothing" is worth doing at all; see
/// `module_item::publish`'s own guard for why a plain legacy article
/// skips calling this entirely.
///
/// Row-locks `module_items` for the duration so two concurrent freezes
/// of the same item can never both compute the same "next version".
pub async fn freeze(
    pool: &PgPool,
    ctx: &AuthContext,
    item_id: Uuid,
    lesson_plan: Option<&serde_json::Value>,
    quiz_config: Option<&serde_json::Value>,
    created_via: &str,
    change_summary: Option<&str>,
) -> Result<i32, AppError> {
    let mut tx = pool.begin().await?;

    let current: Option<i32> = sqlx::query_scalar!(r#"select current_version from module_items where id = $1 for update"#, item_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound("module_item_not_found"))?;

    let next = current.unwrap_or(0) + 1;

    if let Some(prev) = current {
        sqlx::query!(
            r#"update module_item_versions set superseded_at = now() where item_id = $1 and version = $2 and superseded_at is null"#,
            item_id,
            prev,
        )
        .execute(&mut *tx)
        .await?;
    }

    let hash = content_hash(lesson_plan, quiz_config);
    sqlx::query!(
        r#"insert into module_item_versions (item_id, version, lesson_plan, quiz_config, content_hash, created_by, created_via, change_summary)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        item_id,
        next,
        lesson_plan,
        quiz_config,
        hash,
        ctx.user_id,
        created_via,
        change_summary,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(r#"update module_items set current_version = $2 where id = $1"#, item_id, next).execute(&mut *tx).await?;

    tx.commit().await?;
    Ok(next)
}

#[derive(Debug, serde::Serialize)]
pub struct VersionSummary {
    pub version: i32,
    pub content_hash: String,
    pub created_by: Option<Uuid>,
    pub created_via: String,
    pub change_summary: Option<String>,
    pub published_at: chrono::DateTime<chrono::Utc>,
    pub superseded_at: Option<chrono::DateTime<chrono::Utc>>,
}

// Deliberately flat (not `VersionSummary` + `#[serde(flatten)]`) —
// `sqlx::query_as!` maps SELECT columns onto struct fields directly and
// doesn't see through a nested/flattened struct.
#[derive(Debug, serde::Serialize)]
pub struct VersionDetail {
    pub version: i32,
    pub content_hash: String,
    pub created_by: Option<Uuid>,
    pub created_via: String,
    pub change_summary: Option<String>,
    pub published_at: chrono::DateTime<chrono::Utc>,
    pub superseded_at: Option<chrono::DateTime<chrono::Utc>>,
    pub lesson_plan: Option<serde_json::Value>,
    pub quiz_config: Option<serde_json::Value>,
}

// GET /module-items/{id}/versions — newest first, so an author's most
// recent publish is what they see without scrolling.
pub async fn list(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<Vec<VersionSummary>, AppError> {
    crate::services::module_item::can_edit_item(pool, ctx, item_id).await?;
    let rows = sqlx::query_as!(
        VersionSummary,
        r#"select version, content_hash, created_by, created_via, change_summary, published_at, superseded_at
           from module_item_versions where item_id = $1 order by version desc"#,
        item_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// GET /module-items/{id}/versions/{n}
pub async fn get(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, version: i32) -> Result<VersionDetail, AppError> {
    crate::services::module_item::can_edit_item(pool, ctx, item_id).await?;
    let row = sqlx::query_as!(
        VersionDetail,
        r#"select version, content_hash, created_by, created_via, change_summary, published_at, superseded_at, lesson_plan, quiz_config
           from module_item_versions where item_id = $1 and version = $2"#,
        item_id,
        version,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("module_item_version_not_found"))?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identical_content_hashes_the_same_and_different_content_does_not() {
        let a = json!({"sections": [{"title": "A"}]});
        let b = json!({"sections": [{"title": "B"}]});
        assert_eq!(content_hash(Some(&a), None), content_hash(Some(&a), None));
        assert_ne!(content_hash(Some(&a), None), content_hash(Some(&b), None));
    }

    #[test]
    fn lesson_plan_only_and_quiz_config_only_never_collide_even_with_the_same_bytes() {
        // Without a separator between the two fields, hashing
        // `lesson_plan.to_string() + quiz_config.to_string()` could
        // collide across a swap — e.g. ("ab", "") vs ("a", "b").
        let same_text = json!("ab");
        let split_a = json!("a");
        let split_b = json!("b");
        assert_ne!(content_hash(Some(&same_text), None), content_hash(Some(&split_a), Some(&split_b)));
    }

    #[test]
    fn both_absent_is_stable_and_distinct_from_any_real_content() {
        let empty_string = json!("");
        assert_eq!(content_hash(None, None), content_hash(None, None));
        assert_ne!(content_hash(None, None), content_hash(Some(&empty_string), None));
    }
}
