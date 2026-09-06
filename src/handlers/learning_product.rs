use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::marketplace::{CreateProductRequest, ListProductsQuery};
use crate::services::learning_product::{self, ProductListResponse, ProductPageResponse, ProductResponse};
use crate::state::AppState;

// POST /tutors/me/products
pub async fn post_product(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    ValidatedJson(body): ValidatedJson<CreateProductRequest>,
) -> Result<(StatusCode, Json<ProductResponse>), AppError> {
    let result = learning_product::create_product(
        &state.db,
        &ctx,
        &body.r#type,
        &body.title,
        body.description.as_deref().unwrap_or(""),
        body.price_idr,
        body.capacity,
        body.delivery_mode.as_deref().unwrap_or("live_class"),
        body.session_count,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /tutors/{id}/products
pub async fn get_tutor_products(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(tutor_id): Path<Uuid>,
) -> Result<Json<ProductListResponse>, AppError> {
    Ok(Json(learning_product::list_tutor_products(&state.db, &ctx, tutor_id).await?))
}

// GET /products/{id}
pub async fn get_product(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(product_id): Path<Uuid>,
) -> Result<Json<ProductResponse>, AppError> {
    Ok(Json(learning_product::get_product(&state.db, &ctx, product_id).await?))
}

// POST /products/{id}/publish
pub async fn post_publish(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(product_id): Path<Uuid>,
) -> Result<Json<ProductResponse>, AppError> {
    Ok(Json(learning_product::publish_product(&state.db, &ctx, product_id).await?))
}

// GET /products?cursor=&limit=
pub async fn get_products(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    Query(query): Query<ListProductsQuery>,
) -> Result<Json<ProductPageResponse>, AppError> {
    Ok(Json(learning_product::list_published_products(&state.db, query.cursor, query.limit).await?))
}
