use axum::{extract::Multipart, Extension, Json};

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::document_extract::{self, Extracted};
use crate::services::permissions::{require_permission, Action, Resource};

const MAX_BYTES: usize = 10 * 1024 * 1024;

// POST /documents/extract — multipart field "file" (PDF or DOCX) → its
// text. For authors only: it exists to feed the AI composers, and
// parsing arbitrary documents is not something to offer every account.
pub async fn post_extract_document(Extension(ctx): Extension<AuthContext>, mut multipart: Multipart) -> Result<Json<Extracted>, AppError> {
    require_permission(&ctx, Resource::ModuleItem, Action::Create)?;

    let mut file: Option<(String, Option<String>, Vec<u8>)> = None;
    while let Some(field) = multipart.next_field().await.map_err(|e| AppError::UnprocessableEntity("invalid_multipart", e.to_string()))? {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("dokumen").to_string();
        let content_type = field.content_type().map(str::to_string);
        let bytes = field.bytes().await.map_err(|e| AppError::UnprocessableEntity("invalid_multipart", e.to_string()))?;
        file = Some((filename, content_type, bytes.to_vec()));
    }
    let (filename, content_type, bytes) = file.ok_or_else(|| AppError::UnprocessableEntity("file_required", "field \"file\" wajib diisi".to_string()))?;
    if bytes.len() > MAX_BYTES {
        return Err(AppError::PayloadTooLarge("file_too_large"));
    }

    // PDF parsing is CPU-bound; keep it off the async workers.
    let extracted = tokio::task::spawn_blocking(move || document_extract::extract(&filename, content_type.as_deref(), &bytes))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))??;
    Ok(Json(extracted))
}
