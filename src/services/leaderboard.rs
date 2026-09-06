use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::xp;

const WINDOW_DAYS: i64 = 7;

#[derive(Debug, serde::Serialize)]
pub struct LeaderboardItem {
    pub user_id: Uuid,
    pub name: String,
    pub xp: i64,
    pub rank: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct PersonalStanding {
    pub xp: i64,
    pub rank: i64,
    pub percentile: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct WeeklyLeaderboardResponse {
    pub items: Vec<LeaderboardItem>,
    pub me: Option<PersonalStanding>,
}

// GET /leaderboard/weekly?limit= — scope deliberately narrowed to
// Weekly + Personal (no country/region/friends/class/course: no user
// profile field for those, no social-graph/enrollment entities exist
// yet).
pub async fn get_weekly_leaderboard(pool: &PgPool, user_id: Uuid, limit: i64) -> Result<WeeklyLeaderboardResponse, AppError> {
    let since = chrono::Utc::now() - chrono::Duration::days(WINDOW_DAYS);
    let totals = xp::find_weekly_totals(pool, since).await?; // already sorted desc by weekly_xp
    let total_users = totals.len() as i64;

    let items: Vec<LeaderboardItem> = totals
        .iter()
        .take(limit as usize)
        .enumerate()
        .map(|(index, row)| LeaderboardItem { user_id: row.user_id, name: row.name.clone(), xp: row.weekly_xp, rank: index as i64 + 1 })
        .collect();

    let my_index = totals.iter().position(|row| row.user_id == user_id);
    let me = my_index.map(|idx| {
        let rank = idx as i64 + 1;
        let percentile = if total_users > 1 { ((total_users - rank) as f64 / (total_users - 1) as f64 * 100.0).round() as i64 } else { 100 };
        PersonalStanding { xp: totals[idx].weekly_xp, rank, percentile }
    });

    Ok(WeeklyLeaderboardResponse { items, me })
}
