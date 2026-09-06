use axum::{extract::{Query, State}, Extension, Json};
use std::sync::Arc;

use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::models::requests::learning::QueueQuery;
use crate::services::{achievement, daily_mission, league, leaderboard, streak, xp};
use crate::state::AppState;

// GET /me/xp
pub async fn get_xp(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<xp::XpSummary>, AppError> {
    Ok(Json(xp::get_xp_summary(&state.db, ctx.user_id).await?))
}

// GET /me/streak
pub async fn get_streak(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<streak::StreakSummary>, AppError> {
    Ok(Json(streak::get_streak(&state.db, ctx.user_id).await?))
}

// GET /me/achievements
#[derive(Debug, serde::Serialize)]
pub struct AchievementsResponse {
    pub items: Vec<achievement::AchievementSummary>,
}

pub async fn get_achievements(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<AchievementsResponse>, AppError> {
    let items = achievement::get_achievements(&state.db, ctx.user_id).await?;
    Ok(Json(AchievementsResponse { items }))
}

// GET /me/daily-mission
pub async fn get_daily_mission(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<daily_mission::DailyMissionSummary>, AppError> {
    Ok(Json(daily_mission::get_daily_mission(&state.db, ctx.user_id, chrono::Utc::now()).await?))
}

// GET /me/league
pub async fn get_league(State(state): State<Arc<AppState>>, Extension(ctx): Extension<AuthContext>) -> Result<Json<league::LeagueSummary>, AppError> {
    Ok(Json(league::get_league(&state.db, &state.config, ctx.user_id).await?))
}

// GET /leaderboard/weekly?limit=
pub async fn get_weekly_leaderboard(
    State(state): State<Arc<AppState>>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<QueueQuery>,
) -> Result<Json<leaderboard::WeeklyLeaderboardResponse>, AppError> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    Ok(Json(leaderboard::get_weekly_leaderboard(&state.db, ctx.user_id, limit).await?))
}
