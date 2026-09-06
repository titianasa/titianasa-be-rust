use axum::{routing::get, Router};
use std::sync::Arc;

use crate::handlers;
use crate::state::AppState;

pub fn protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/me/xp", get(handlers::gamification::get_xp))
        .route("/me/streak", get(handlers::gamification::get_streak))
        .route("/me/achievements", get(handlers::gamification::get_achievements))
        .route("/me/daily-mission", get(handlers::gamification::get_daily_mission))
        .route("/me/league", get(handlers::gamification::get_league))
        .route("/leaderboard/weekly", get(handlers::gamification::get_weekly_leaderboard))
}
