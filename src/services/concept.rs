use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

pub struct ConceptRow {
    pub id: Uuid,
    pub name: String,
    pub parent_concept_id: Option<Uuid>,
}

pub async fn find(pool: &PgPool, id: Uuid) -> Result<Option<ConceptRow>, AppError> {
    let row = sqlx::query_as!(ConceptRow, r#"select id, name, parent_concept_id from concepts where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub struct ConceptWithDepth {
    pub id: Uuid,
    pub name: String,
    pub parent_concept_id: Option<Uuid>,
    pub depth: i32,
}

// Port of concept_repository.ts's findDescendantsUpTo — bounded to
// maxDepth levels below `id` (1 = direct children only, 2 = children +
// grandchildren), carrying each row's depth.
pub async fn find_descendants_up_to(pool: &PgPool, id: Uuid, max_depth: i32) -> Result<Vec<ConceptWithDepth>, AppError> {
    let rows = sqlx::query_as!(
        ConceptWithDepth,
        r#"WITH RECURSIVE descendants AS (
             SELECT c.id, c.name, c.parent_concept_id, 1 AS depth FROM concepts c WHERE c.parent_concept_id = $1
             UNION ALL
             SELECT c.id, c.name, c.parent_concept_id, d.depth + 1 FROM concepts c
             JOIN descendants d ON c.parent_concept_id = d.id
             WHERE d.depth < $2
           )
           SELECT id as "id!", name as "name!", parent_concept_id, depth as "depth!" FROM descendants ORDER BY depth"#,
        id,
        max_depth,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub struct PrerequisiteEdge {
    pub concept_id: Uuid,
    pub name: String,
}

// Direct (1-level) prerequisites of `id` — no recursion, per the MVP
// scope ("prerequisite untuk 1 level").
pub async fn find_prerequisites(pool: &PgPool, id: Uuid) -> Result<Vec<PrerequisiteEdge>, AppError> {
    let rows = sqlx::query_as!(
        PrerequisiteEdge,
        r#"select c.id as concept_id, c.name from concept_prerequisites cp
           inner join concepts c on c.id = cp.prerequisite_concept_id
           where cp.concept_id = $1"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// Every concept transitively required by `id` — the prerequisite-graph
// equivalent of findAncestors, used only for the cycle check below.
// Safe from infinite recursion only because add_prerequisite is the
// sole writer and always runs this check first (data is acyclic by
// construction).
async fn find_transitive_prerequisite_ids(pool: &PgPool, id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query_scalar!(
        r#"WITH RECURSIVE prereqs AS (
             SELECT cp.prerequisite_concept_id AS id FROM concept_prerequisites cp WHERE cp.concept_id = $1
             UNION ALL
             SELECT cp.prerequisite_concept_id AS id FROM concept_prerequisites cp
             JOIN prereqs p ON cp.concept_id = p.id
           )
           SELECT DISTINCT id as "id!" FROM prereqs"#,
        id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn insert_prerequisite(pool: &PgPool, concept_id: Uuid, prerequisite_concept_id: Uuid) -> Result<(), AppError> {
    sqlx::query!(
        r#"insert into concept_prerequisites (concept_id, prerequisite_concept_id) values ($1, $2)
           on conflict do nothing"#,
        concept_id,
        prerequisite_concept_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn delete_prerequisite(pool: &PgPool, concept_id: Uuid, prerequisite_concept_id: Uuid) -> Result<(), AppError> {
    sqlx::query!(
        r#"delete from concept_prerequisites where concept_id = $1 and prerequisite_concept_id = $2"#,
        concept_id,
        prerequisite_concept_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub struct PrerequisiteListItem {
    pub concept_id: Uuid,
    pub name: String,
}

// GET /concepts/{id}/prerequisites
pub async fn list_prerequisites(pool: &PgPool, concept_id: Uuid) -> Result<Vec<PrerequisiteListItem>, AppError> {
    find(pool, concept_id).await?.ok_or(AppError::NotFound("concept_not_found"))?;
    let rows = find_prerequisites(pool, concept_id).await?;
    Ok(rows.into_iter().map(|r| PrerequisiteListItem { concept_id: r.concept_id, name: r.name }).collect())
}

// POST /concepts/{id}/prerequisites. ADR-0007: Postgres doesn't enforce
// acyclicity on this graph declaratively, so it's checked here before
// every write — a concept can't become its own prerequisite, directly
// or transitively.
pub async fn add_prerequisite(pool: &PgPool, concept_id: Uuid, prerequisite_concept_id: Uuid) -> Result<(), AppError> {
    if prerequisite_concept_id == concept_id {
        return Err(AppError::UnprocessableEntity(
            "concept_prerequisite_cycle",
            "a concept cannot be its own prerequisite".to_string(),
        ));
    }

    find(pool, concept_id).await?.ok_or(AppError::NotFound("concept_not_found"))?;
    find(pool, prerequisite_concept_id).await?.ok_or(AppError::NotFound("concept_not_found"))?;

    let transitive = find_transitive_prerequisite_ids(pool, prerequisite_concept_id).await?;
    if transitive.contains(&concept_id) {
        return Err(AppError::UnprocessableEntity(
            "concept_prerequisite_cycle",
            "the proposed prerequisite already depends on this concept, directly or transitively".to_string(),
        ));
    }

    insert_prerequisite(pool, concept_id, prerequisite_concept_id).await
}

// DELETE /concepts/{id}/prerequisites/{prerequisite_concept_id}
pub async fn remove_prerequisite(pool: &PgPool, concept_id: Uuid, prerequisite_concept_id: Uuid) -> Result<(), AppError> {
    delete_prerequisite(pool, concept_id, prerequisite_concept_id).await
}
