use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::Response,
    Extension, Json,
};
use std::sync::Arc;
use uuid::Uuid;

use crate::errors::AppError;
use crate::extract::ValidatedJson;
use crate::models::auth::AuthContext;
use crate::models::requests::drive::ShareRequest;
use crate::models::requests::module::{
    AddModulePrerequisiteRequest, CreateModuleItemRequest, CreateModuleRequest, ReorderModuleItemsRequest, ReorderModulesRequest, UpdateModuleItemContentRequest,
    UpdateModuleItemMetaRequest, UpdateModuleItemSubjectRequest, UpdateModuleRequest, UpdateQuizConfigRequest,
};
use crate::models::responses::module::{
    ListModulesResponse, ModuleAncestorsResponse, ModulePrerequisitesResponse, ModuleResponse, ModuleItemDetailResponse, ModuleItemStatusResponse, ModuleItemTreeResponse,
};
use crate::services::{module, module_item, module_completion, resource_share};
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct ParentIdQuery {
    pub parent_id: Option<Uuid>,
}

// GET /modules?parent_id=
pub async fn get_children(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Query(query): Query<ParentIdQuery>) -> Result<Json<ListModulesResponse>, AppError> {
    Ok(Json(module::list_children(&state.db, query.parent_id).await?))
}

// POST /modules
pub async fn post_module(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<CreateModuleRequest>) -> Result<(StatusCode, Json<ModuleResponse>), AppError> {
    let result = module::create(&state.db, &ctx, body).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// GET /modules/{id}
pub async fn get_module(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleResponse>, AppError> {
    Ok(Json(module::get(&state.db, id).await?))
}

// PATCH /modules/{id}
pub async fn patch_module(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>, ValidatedJson(body): ValidatedJson<UpdateModuleRequest>) -> Result<Json<ModuleResponse>, AppError> {
    Ok(Json(module::update(&state.db, &ctx, id, body).await?))
}

// DELETE /modules/{id}
pub async fn delete_module(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    module::delete(&state.db, &ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /modules/{id}/ancestors
pub async fn get_ancestors(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleAncestorsResponse>, AppError> {
    Ok(Json(module::list_ancestors(&state.db, id).await?))
}

// POST /modules/reorder
pub async fn post_reorder_modules(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<ReorderModulesRequest>) -> Result<StatusCode, AppError> {
    module::reorder(&state.db, &ctx, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /modules/{id}/prerequisites
pub async fn get_prerequisites(State(state): State<Arc<AppState>>, Extension(_ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModulePrerequisitesResponse>, AppError> {
    Ok(Json(module::list_prerequisites(&state.db, id).await?))
}

// POST /modules/{id}/prerequisites
pub async fn post_prerequisite(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<AddModulePrerequisiteRequest>,
) -> Result<StatusCode, AppError> {
    module::add_prerequisite(&state.db, &ctx, id, body).await?;
    Ok(StatusCode::CREATED)
}

// DELETE /modules/{id}/prerequisites/{prerequisite_module_id}
pub async fn delete_prerequisite(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path((id, prerequisite_module_id)): Path<(Uuid, Uuid)>) -> Result<StatusCode, AppError> {
    module::remove_prerequisite(&state.db, &ctx, id, prerequisite_module_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /modules/{id}/items
pub async fn get_items(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemTreeResponse>, AppError> {
    // Authors see the tree as built; learners see it with their own locks.
    let author = crate::services::permissions::is_allowed(ctx.role.as_deref(), crate::services::permissions::Resource::ModuleItem, crate::services::permissions::Action::ViewUnpublished);
    Ok(Json(module_item::get_tree(&state.db, id, if author { None } else { Some(ctx.user_id) }).await?))
}

// POST /modules/{id}/items
pub async fn post_item(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<CreateModuleItemRequest>,
) -> Result<(StatusCode, Json<ModuleItemStatusResponse>), AppError> {
    let result = module_item::create(&state.db, &ctx, id, body).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// PATCH /module-items/{id}
pub async fn patch_item(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateModuleItemMetaRequest>,
) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::update_meta(&state.db, &ctx, id, body).await?))
}

// POST /module-items/reorder
pub async fn post_reorder_items(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, ValidatedJson(body): ValidatedJson<ReorderModuleItemsRequest>) -> Result<StatusCode, AppError> {
    module_item::reorder(&state.db, &ctx, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /module-items/{id}
pub async fn get_item(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemDetailResponse>, AppError> {
    Ok(Json(module_item::get_detail(&state.db, &ctx, id).await?))
}

// PUT /module-items/{id}
pub async fn put_item(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateModuleItemContentRequest>,
) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::update_content(&state.db, &ctx, id, &body.content, &body.format).await?))
}

// PATCH /module-items/{id}/quiz-config — the Quiz Builder's save action.
pub async fn patch_quiz_config(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateQuizConfigRequest>,
) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::update_quiz_config(&state.db, &ctx, id, body.quiz_config).await?))
}

// PATCH /module-items/{id}/lesson-plan — the Modul Belajar editor's save.
pub async fn patch_lesson_plan(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<crate::models::requests::module::UpdateLessonPlanRequest>,
) -> Result<Json<crate::models::responses::module::LessonPlanSaveResponse>, AppError> {
    Ok(Json(module_item::update_lesson_plan(&state.db, &ctx, id, body.lesson_plan, body.ai_generated).await?))
}

// PATCH /module-items/{id}/subject — "ganti mapel item ini, tidak
// mengikuti mapel modul induknya". null clears the override.
pub async fn patch_item_subject(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<UpdateModuleItemSubjectRequest>,
) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::update_subject(&state.db, &ctx, id, body.subject_id).await?))
}

// DELETE /module-items/{id}
pub async fn delete_item(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    module_item::delete(&state.db, &ctx, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// POST /module-items/{id}/duplicate
pub async fn post_duplicate_item(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<(StatusCode, Json<ModuleItemStatusResponse>), AppError> {
    let result = module_item::duplicate(&state.db, &ctx, id).await?;
    Ok((StatusCode::CREATED, Json(result)))
}

// POST /module-items/{id}/submit-review
pub async fn post_submit_review(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::submit_for_review(&state.db, &ctx, id).await?))
}

// POST /module-items/{id}/publish
pub async fn post_publish(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::publish(&state.db, &ctx, id).await?))
}

// POST /module-items/{id}/reject
pub async fn post_reject(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<ModuleItemStatusResponse>, AppError> {
    Ok(Json(module_item::reject(&state.db, &ctx, id).await?))
}

// --- Curriculum labels (migration 0039) ---

#[derive(serde::Deserialize)]
pub struct AttachLabelRequest {
    pub kind: String,
    pub value: String,
    /// "human" (default) or "ai" — lets an autonomous curriculum agent's
    /// labels be reviewed separately from a curator's.
    pub source: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct ByLabelQuery {
    pub kind: String,
    pub value: String,
}

// POST /modules/{id}/labels
pub async fn post_module_label(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<AttachLabelRequest>,
) -> Result<(StatusCode, Json<crate::services::module_label::ModuleLabel>), AppError> {
    let source = body.source.as_deref().unwrap_or("human");
    let label = crate::services::module_label::attach(&state.db, &ctx, id, &body.kind, &body.value, source).await?;
    Ok((StatusCode::CREATED, Json(label)))
}

// GET /modules/{id}/labels
pub async fn get_module_labels(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<crate::services::module_label::ModuleLabel>>, AppError> {
    Ok(Json(crate::services::module_label::list_for_module(&state.db, id).await?))
}

// DELETE /modules/{id}/labels/{label_id}
pub async fn delete_module_label(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, label_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    crate::services::module_label::detach(&state.db, &ctx, id, label_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /modules/by-label?kind=&value= — assemble a learning path from
// the master library instead of duplicating modules into it.
pub async fn get_modules_by_label(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    Query(q): Query<ByLabelQuery>,
) -> Result<Json<Vec<crate::services::module_label::LabeledModule>>, AppError> {
    Ok(Json(crate::services::module_label::find_by_label(&state.db, &q.kind, &q.value).await?))
}

// GET /modules/label-rollup?parent_id= — every direct child's subtree
// labels, unioned. Reuses ParentIdQuery so it takes exactly the same
// parameter as the GET /modules listing it decorates.
pub async fn get_label_rollup(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
    Query(query): Query<ParentIdQuery>,
) -> Result<Json<crate::services::module_label::ListRollupResponse>, AppError> {
    Ok(Json(crate::services::module_label::rollup_for_children(&state.db, query.parent_id).await?))
}

// GET /public/curriculum-preview?labels=a:b,c:d — no auth: this is what
// the pre-signup onboarding shows a visitor.
#[derive(Debug, serde::Deserialize)]
pub struct PreviewQuery {
    pub labels: String,
}

pub async fn get_curriculum_preview(
    State(state): State<Arc<AppState>>,
    Query(q): Query<PreviewQuery>,
) -> Result<Json<Vec<crate::services::module_label::LabelPreview>>, AppError> {
    let mut out = Vec::new();
    // Capped so an anonymous caller can't fan this into a heavy query.
    for spec in q.labels.split(',').filter(|s| !s.trim().is_empty()).take(6) {
        let Some((kind, value)) = spec.split_once(':') else { continue };
        out.push(crate::services::module_label::preview(&state.db, kind.trim(), value.trim()).await?);
    }
    Ok(Json(out))
}

// GET /public/learning-paths — no auth: the landing page's swipeable
// card carousel, each card's accordion built from real curriculum data.
pub async fn get_landing_paths(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::services::module_label::LearningPathPreview>>, AppError> {
    Ok(Json(crate::services::module_label::list_landing_paths(&state.db).await?))
}

#[derive(serde::Deserialize)]
pub struct LandingChildrenQuery {
    pub parent_id: Uuid,
}

// GET /public/learning-paths/children?parent_id= — no auth. The landing
// payload is deliberately shallow; this is how the explorer fills a
// folder in when the visitor actually opens it.
pub async fn get_landing_children(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LandingChildrenQuery>,
) -> Result<Json<Vec<crate::services::module_label::PathNode>>, AppError> {
    Ok(Json(crate::services::module_label::landing_children(&state.db, q.parent_id).await?))
}

// POST /learning-paths/{id}/sync — bring one path in line with its spec.
// Idempotent: running it again after no curriculum change reports zero
// inserted and zero removed.
pub async fn post_sync_learning_path(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::services::learning_path::SyncReport>, AppError> {
    Ok(Json(crate::services::learning_path::sync_path(&state.db, &ctx, id).await?))
}

// POST /learning-paths/sync — every path that declares a spec.
pub async fn post_sync_learning_paths(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<crate::services::learning_path::SyncReport>>, AppError> {
    Ok(Json(crate::services::learning_path::sync_all(&state.db, &ctx).await?))
}

// GET /learning-paths/validate — the curriculum invariants, as data.
pub async fn get_learning_path_validation(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
) -> Result<Json<crate::services::learning_path::ValidationReport>, AppError> {
    Ok(Json(crate::services::learning_path::validate(&state.db).await?))
}

// GET /module-labels/summary — coverage map across the whole library.
pub async fn get_label_summary(
    State(state): State<Arc<AppState>>,
    Extension(_ctx): Extension<AuthContext>,
) -> Result<Json<Vec<crate::services::module_label::LabelSummary>>, AppError> {
    Ok(Json(crate::services::module_label::summary(&state.db).await?))
}

// GET /module-items/{id}/pending-reviews — Manual-mode quiz question
// groups awaiting a teacher's score, across every submitted attempt on
// this item.
pub async fn get_pending_reviews(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<Vec<crate::services::quiz_attempt::PendingReviewRow>>, AppError> {
    Ok(Json(crate::services::quiz_attempt::list_pending_reviews(&state.db, &ctx, id).await?))
}

// GET /module-items/{id}/speaking-prompt-audio — returns raw audio
// bytes, not JSON, matching speaking_room.rs's post_tts precedent.
pub async fn get_speaking_prompt_audio(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Response, AppError> {
    let prompt = module_item::get_speaking_prompt_text(&state.db, &ctx, id).await?;
    let result = state
        .ai_provider
        .synthesize_speech(&prompt.text, &state.config.ai_tts_default_voice, &state.config.ai_tts_model)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(Response::builder().status(StatusCode::OK).header(header::CONTENT_TYPE, result.content_type).body(Body::from(result.bytes)).unwrap())
}

// GET /module-items/{id}/completion-status — §37 (ADR-0012 P21-002).
pub async fn get_completion_status(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<module_completion::ItemCompletionStatusResponse>, AppError> {
    Ok(Json(module_completion::get_completion_status(&state.db, &state.config, ctx.user_id, id).await?))
}

// POST /module-items/{id}/skip-completion
pub async fn post_skip_completion(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<module_completion::SkipCompletionResponse>, AppError> {
    Ok(Json(module_completion::skip_completion(&state.db, &state.config, ctx.user_id, id).await?))
}

// Phase 31 (P31-008) — collaborator sharing. `ShareResponse` mirrors
// handlers/asset.rs's own local copy (this codebase's convention: a
// response type lives next to the handler that returns it, not a
// shared models file, when nothing else needs it).
#[derive(serde::Serialize)]
pub struct ShareResponse {
    pub id: Uuid,
    pub principal_type: String,
    pub principal_id: String,
    pub permission: String,
}

// GET /module-items/{id}/shares
pub async fn get_item_shares(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path(id): Path<Uuid>) -> Result<Json<Vec<ShareResponse>>, AppError> {
    let shares = resource_share::list_module_item_shares(&state.db, &ctx, id).await?;
    Ok(Json(shares.into_iter().map(|s| ShareResponse { id: s.id, principal_type: s.principal_type, principal_id: s.principal_id, permission: s.permission }).collect()))
}

// POST /module-items/{id}/shares
pub async fn post_item_share(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    ValidatedJson(body): ValidatedJson<ShareRequest>,
) -> Result<(StatusCode, Json<ShareResponse>), AppError> {
    let share = resource_share::share_module_item(&state.db, &ctx, id, &body.principal_type, &body.principal_id, &body.permission).await?;
    Ok((StatusCode::CREATED, Json(ShareResponse { id: share.id, principal_type: share.principal_type, principal_id: share.principal_id, permission: share.permission })))
}

// DELETE /module-items/{id}/shares/{share_id}
pub async fn delete_item_share(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>, Path((_id, share_id)): Path<(Uuid, Uuid)>) -> Result<StatusCode, AppError> {
    resource_share::unshare_module_item(&state.db, &ctx, share_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// GET /quiz-subtypes — the whole subtype registry (id, family,
// category, grading mode, AI output shape, label/description, context
// requirements). Served rather than duplicated in the frontend: the
// authoring picker, the QA hints and the generator all need the same
// 40-row table, and a hand-maintained second copy in TypeScript drifted
// the moment a subtype gained a field.
pub async fn get_quiz_subtypes() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "items": crate::services::quiz_subtype::SUBTYPES }))
}

// GET /quiz-taxonomy — the difficulty/Bloom vocabulary, with both the
// Indonesian and the English label for every level, so the Studio UI
// and any future importer read the levels off one source of truth
// instead of each hard-coding its own C1..C6 table.
pub async fn get_quiz_taxonomy() -> axum::Json<crate::services::quiz_taxonomy::TaxonomyVocabulary> {
    axum::Json(crate::services::quiz_taxonomy::vocabulary())
}

// GET /quiz-templates — the exam blueprint catalogue. Structure only:
// applying one gives an empty, correctly-shaped paper the author then
// fills in (each group carries a `context_prompt` steering generation).
pub async fn get_quiz_templates() -> axum::Json<serde_json::Value> {
    use crate::services::quiz_template;
    let items: Vec<serde_json::Value> = quiz_template::TEMPLATES.iter().map(template_summary_json).collect();
    axum::Json(serde_json::json!({ "folders": quiz_template::FOLDERS, "items": items }))
}

// GET /quiz-templates/{id} — one template's full detail (Phase 38's
// template browser detail panel). The list above already embeds every
// template's sections/groups too (nothing here is hidden from it), so
// this exists for the "fetch one, lazily, once selected" shape a
// browser UI wants rather than for data the list doesn't have.
pub async fn get_quiz_template(Path(id): Path<String>) -> Result<axum::Json<serde_json::Value>, AppError> {
    use crate::services::quiz_template;
    let template = quiz_template::find(&id).ok_or(AppError::NotFound("quiz_template_not_found"))?;
    Ok(axum::Json(template_summary_json(template)))
}

fn template_summary_json(t: &crate::services::quiz_template::QuizTemplate) -> serde_json::Value {
    let mut value = serde_json::to_value(t).expect("template serializes");
    value["question_count"] = serde_json::json!(t.question_count());
    value["duration_label"] = serde_json::json!(t.duration_label());
    value["difficulty"] = serde_json::json!(t.difficulty());
    value["tags"] = serde_json::json!(t.tags());
    value
}

// POST /quiz-templates/{id}/apply — the blueprint turned into a real
// quiz_config, validated before it is handed back.
pub async fn post_apply_quiz_template(Path(id): Path<String>) -> Result<axum::Json<serde_json::Value>, AppError> {
    use crate::services::{quiz_config_schema, quiz_template};
    let template = quiz_template::find(&id).ok_or(AppError::NotFound("quiz_template_not_found"))?;
    let config = quiz_template::apply(template);
    quiz_config_schema::validate_structure(&config)?;
    Ok(axum::Json(config))
}
