use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

// The `subjects` table is a coarser taxonomy than the module tree — e.g.
// Akidah Akhlak/Fiqih/Al-Qur'an Hadist/SKI all share one "Pendidikan Agama
// Islam" row, and 5 foreign languages share one "Bahasa Asing" row — but
// it's the FK every module, program and question bank actually points at,
// so Content Studio needs to be able to list and read it.
//
// This didn't exist until now: every "new module/program/question bank"
// form in Content Studio silently defaulted to one hardcoded English
// subject id (see titian-web's `DEFAULT_SUBJECT_ID`), a leftover from
// when English was the only subject in the system. Now that the library
// covers 48 subjects, that default silently mislabels everything created
// through Studio — this is what lets a picker replace it.

pub struct SubjectRow {
    pub id: Uuid,
    pub code: String,
    pub name: String,
}

pub async fn list(pool: &PgPool) -> Result<Vec<SubjectRow>, AppError> {
    let rows = sqlx::query_as!(SubjectRow, r#"select id, code, name from subjects order by name asc"#)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn find(pool: &PgPool, id: Uuid) -> Result<Option<SubjectRow>, AppError> {
    let row = sqlx::query_as!(SubjectRow, r#"select id, code, name from subjects where id = $1"#, id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

// Same slugify + retry-with-suffix-on-collision shape as
// organization.rs's create_organization — this is the answer to "what
// if the subject I need isn't in the list": a curriculum developer
// types it once in the picker (subject-select.tsx's "+ Tambah mata
// pelajaran baru"), it's created here, and it's immediately available
// to every other module/program/question bank form afterward.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    let slug: String = slug.chars().take(60).collect();
    if slug.is_empty() { "mata-pelajaran".to_string() } else { slug }
}

pub async fn create(pool: &PgPool, name: &str) -> Result<SubjectRow, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::UnprocessableEntity("name_required", "nama mata pelajaran wajib diisi".to_string()));
    }
    // A name that only differs by case/whitespace from an existing
    // subject is almost always the SAME subject, not a new one — reuse
    // it instead of creating a confusing near-duplicate.
    if let Some(existing) = sqlx::query_as!(
        SubjectRow,
        r#"select id, code, name from subjects where lower(name) = lower($1)"#,
        name,
    )
    .fetch_optional(pool)
    .await?
    {
        return Ok(existing);
    }

    let base_code = slugify(name);
    if let Some(row) = sqlx::query_as!(
        SubjectRow,
        r#"insert into subjects (code, name) values ($1, $2)
           on conflict (code) do nothing
           returning id, code, name"#,
        base_code,
        name,
    )
    .fetch_optional(pool)
    .await?
    {
        return Ok(row);
    }
    let suffixed_code = format!("{base_code}-{}", &Uuid::new_v4().simple().to_string()[..4]);
    let row = sqlx::query_as!(
        SubjectRow,
        r#"insert into subjects (code, name) values ($1, $2) returning id, code, name"#,
        suffixed_code,
        name,
    )
    .fetch_one(pool)
    .await?;
    Ok(row)
}
