use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::block_schema::{self, BlockRef};

pub struct NewContentBlock {
    pub r#type: String,
    pub order_index: i32,
    pub data: serde_json::Value,
    pub raw_source: Option<String>,
}

// Port of question_repository.ts/assessment_repository.ts's
// findExistingIds — minimal existence-check queries against tables not
// otherwise ported yet (question/assessment authoring is R4), needed
// here only to validate question_embed/assessment_embed references.
async fn find_existing_question_ids(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<Uuid>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_scalar!(r#"select id from questions where id = any($1)"#, ids).fetch_all(pool).await?;
    Ok(rows)
}

async fn find_existing_assessment_ids(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<Uuid>, AppError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_scalar!(r#"select id from assessments where id = any($1)"#, ids).fetch_all(pool).await?;
    Ok(rows)
}

// Port of content_block_service.ts's validateBlocks. Validates every
// block's structural schema and, for question_embed/assessment_embed
// blocks, that the referenced ids actually exist (batched).
pub async fn validate_blocks(pool: &PgPool, blocks: &[NewContentBlock]) -> Result<(), AppError> {
    for block in blocks {
        block_schema::validate(&block.r#type, &block.data)?;
    }

    let block_refs: Vec<BlockRef> = blocks.iter().map(|b| BlockRef { r#type: &b.r#type, data: &b.data }).collect();

    let referenced_question_ids = block_schema::extract_question_embed_ids(&block_refs);
    if !referenced_question_ids.is_empty() {
        let ids: Vec<Uuid> = referenced_question_ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
        let existing: std::collections::HashSet<Uuid> = find_existing_question_ids(pool, &ids).await?.into_iter().collect();
        let missing: Vec<&String> = referenced_question_ids
            .iter()
            .filter(|id| Uuid::parse_str(id).map(|u| !existing.contains(&u)).unwrap_or(true))
            .collect();
        if !missing.is_empty() {
            let joined = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
            return Err(AppError::UnprocessableEntity(
                "question_embed_not_found",
                format!("referenced question_id(s) do not exist: {joined}"),
            ));
        }
    }

    let referenced_assessment_ids = block_schema::extract_assessment_embed_ids(&block_refs);
    if !referenced_assessment_ids.is_empty() {
        let ids: Vec<Uuid> = referenced_assessment_ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
        let existing: std::collections::HashSet<Uuid> = find_existing_assessment_ids(pool, &ids).await?.into_iter().collect();
        let missing: Vec<&String> = referenced_assessment_ids
            .iter()
            .filter(|id| Uuid::parse_str(id).map(|u| !existing.contains(&u)).unwrap_or(true))
            .collect();
        if !missing.is_empty() {
            let joined = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
            return Err(AppError::UnprocessableEntity(
                "assessment_embed_not_found",
                format!("referenced assessment_id(s) do not exist: {joined}"),
            ));
        }
    }

    Ok(())
}

// Port of content_block_service.ts's validateAndReplaceBlocks — the one
// write path item authoring goes through, so validation can never be
// bypassed by writing directly to the DB layer.
pub async fn validate_and_replace_blocks(pool: &PgPool, item_id: Uuid, blocks: Vec<NewContentBlock>) -> Result<(), AppError> {
    validate_blocks(pool, &blocks).await?;

    let mut tx = pool.begin().await?;
    sqlx::query!(r#"delete from content_blocks where item_id = $1"#, item_id).execute(&mut *tx).await?;
    for block in blocks {
        sqlx::query!(
            r#"insert into content_blocks (item_id, type, order_index, data, raw_source)
               values ($1, $2, $3, $4, $5)"#,
            item_id,
            block.r#type,
            block.order_index,
            block.data,
            block.raw_source,
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
