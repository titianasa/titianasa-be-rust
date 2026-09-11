// "Keamanan Ujian (Proctor)" — per-item proctor rules with inheritance,
// ported from parelabs' proctor_resolver + ProctorConfigSection, and the
// proctored sittings of module quizzes.
//
// Config cascade: module → root folder → … → item. Each level stores only
// what it overrides; a key that is absent inherits, `false` switches an
// inherited rule off, and the leaf wins. So a rule set on a folder covers
// every quiz inside it unless one of them says otherwise.
//
// Keys:
//   enabled, require_webcam, require_id_card, block_paste,
//   force_fullscreen, disable_spellcheck          booleans
//   snapshot_interval_sec                         webcam snapshot period
//   geofence { enabled, max_violations, centers[{label, lat, lng, radius_m}] }
//   auto_lock_rules [{ type, after_count }]       lock after N events of a type

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;
use crate::errors::AppError;
use crate::models::auth::AuthContext;
use crate::services::permissions::{require_permission, Action, Resource};
use crate::services::storage::AssetStorage;
use crate::services::{item_progress, module_item};

/// Events the exam page reports.
const EVENT_TYPES: [&str; 11] = [
    "tab_hidden",
    "tab_visible",
    "window_blur",
    "fullscreen_exit",
    "fullscreen_enter",
    "paste_attempt",
    "camera_off",
    "snapshot",
    "id_card",
    "location",
    "submitted",
];

/// Events an auto-lock rule may count, with the reason shown when it trips.
pub const LOCKABLE: [(&str, &str); 6] = [
    ("tab_hidden", "Berpindah tab"),
    ("window_blur", "Meninggalkan jendela ujian"),
    ("fullscreen_exit", "Keluar dari layar penuh"),
    ("paste_attempt", "Mencoba menempel teks"),
    ("camera_off", "Kamera dimatikan"),
    ("location_violation", "Keluar dari lokasi yang diizinkan"),
];

const EARTH_RADIUS_M: f64 = 6_371_000.0;

fn invalid(detail: &str) -> AppError {
    AppError::UnprocessableEntity("invalid_proctor_config", detail.to_string())
}

/// Leaf-wins deep merge. A `null` in the overlay removes the key — the
/// escape hatch for a child to clear something its parent set.
pub fn deep_merge(base: &mut Value, overlay: &Value) {
    let (Value::Object(base_map), Value::Object(over_map)) = (base, overlay) else { return };
    for (key, value) in over_map {
        if value.is_null() {
            base_map.remove(key);
            continue;
        }
        match (base_map.get_mut(key), value) {
            (Some(existing @ Value::Object(_)), Value::Object(_)) => deep_merge(existing, value),
            _ => {
                base_map.insert(key.clone(), value.clone());
            }
        }
    }
}

fn number_in(value: &Value, min: f64, max: f64) -> bool {
    value.as_f64().is_some_and(|n| (min..=max).contains(&n))
}

pub fn validate_config(value: &Value) -> Result<(), AppError> {
    let Some(obj) = value.as_object() else { return Err(invalid("config harus berupa objek")) };
    for (key, v) in obj {
        if v.is_null() {
            continue;
        }
        match key.as_str() {
            "enabled" | "require_webcam" | "require_id_card" | "block_paste" | "force_fullscreen" | "disable_spellcheck" => {
                if !v.is_boolean() {
                    return Err(invalid(&format!("{key} harus true/false")));
                }
            }
            "snapshot_interval_sec" => {
                if !number_in(v, 5.0, 3600.0) {
                    return Err(invalid("snapshot_interval_sec harus 5-3600 detik"));
                }
            }
            "geofence" => {
                let Some(geo) = v.as_object() else { return Err(invalid("geofence harus berupa objek")) };
                for (gk, gv) in geo {
                    match gk.as_str() {
                        "enabled" if gv.is_boolean() || gv.is_null() => {}
                        "max_violations" if gv.is_null() || number_in(gv, 1.0, 100.0) => {}
                        "centers" if gv.is_null() => {}
                        "centers" => {
                            let Some(centers) = gv.as_array() else { return Err(invalid("geofence.centers harus berupa daftar")) };
                            for c in centers {
                                let ok = c.get("lat").is_some_and(|x| number_in(x, -90.0, 90.0))
                                    && c.get("lng").is_some_and(|x| number_in(x, -180.0, 180.0))
                                    && c.get("radius_m").is_some_and(|x| number_in(x, 10.0, 50_000.0));
                                if !ok {
                                    return Err(invalid("setiap titik geofence butuh lat, lng, dan radius_m 10-50000 meter"));
                                }
                            }
                        }
                        _ => return Err(invalid(&format!("geofence.{gk} tidak dikenal atau nilainya salah"))),
                    }
                }
            }
            "auto_lock_rules" => {
                let Some(rules) = v.as_array() else { return Err(invalid("auto_lock_rules harus berupa daftar")) };
                for rule in rules {
                    let kind = rule.get("type").and_then(Value::as_str).unwrap_or_default();
                    if !LOCKABLE.iter().any(|(k, _)| *k == kind) {
                        return Err(invalid(&format!("jenis pelanggaran \"{kind}\" tidak dikenal")));
                    }
                    if !rule.get("after_count").is_some_and(|x| number_in(x, 1.0, 100.0)) {
                        return Err(invalid("after_count harus 1-100"));
                    }
                }
            }
            other => return Err(invalid(&format!("pengaturan \"{other}\" tidak dikenal"))),
        }
    }
    Ok(())
}

fn flag(config: &Value, key: &str) -> bool {
    config.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// (id, title, node_type, proctor_config, depth) from the item up to its
/// root; depth 0 is the item itself.
async fn chain_rows(pool: &PgPool, item_id: Uuid) -> Result<Vec<(Uuid, String, String, Option<Value>, i32)>, AppError> {
    // Depth-bounded so an accidental parent cycle cannot loop forever.
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<Value>, i32)>(
        r#"with recursive chain as (
               select id, parent_id, title, node_type, proctor_config, 0 as depth from module_items where id = $1
               union all
               select mi.id, mi.parent_id, mi.title, mi.node_type, mi.proctor_config, c.depth + 1
               from module_items mi join chain c on mi.id = c.parent_id where c.depth < 10
           )
           select id, title, node_type, proctor_config, depth from chain order by depth asc"#,
    )
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

struct ModuleConfigRow {
    id: Uuid,
    title: String,
    proctor_config: Option<Value>,
}

async fn module_of(pool: &PgPool, item_id: Uuid) -> Result<Option<ModuleConfigRow>, AppError> {
    Ok(sqlx::query_as!(
        ModuleConfigRow,
        r#"select m.id, m.title, m.proctor_config from module_items i join modules m on m.id = i.module_id where i.id = $1"#,
        item_id,
    )
    .fetch_optional(pool)
    .await?)
}

fn usable(config: &Option<Value>) -> Option<&Value> {
    config.as_ref().filter(|v| v.as_object().is_some_and(|o| !o.is_empty()))
}

/// The config actually in force for an item.
pub async fn effective_for_item(pool: &PgPool, item_id: Uuid) -> Result<Value, AppError> {
    let mut effective = json!({});
    if let Some(module) = module_of(pool, item_id).await? {
        if let Some(cfg) = usable(&module.proctor_config) {
            deep_merge(&mut effective, cfg);
        }
    }
    for (_, _, _, cfg, _) in chain_rows(pool, item_id).await?.iter().rev() {
        if let Some(cfg) = usable(cfg) {
            deep_merge(&mut effective, cfg);
        }
    }
    Ok(effective)
}

#[derive(Debug, Serialize)]
pub struct ChainNode {
    pub id: Uuid,
    pub title: String,
    /// "module", "section" or "item".
    pub node_type: String,
    pub depth_from_leaf: i32,
    pub has_own_config: bool,
    pub config: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ItemProctorConfigResponse {
    pub item_id: Uuid,
    /// What THIS item overrides (null: everything inherited).
    pub raw: Option<Value>,
    pub effective: Value,
    /// Root first (the module), the item itself last.
    pub inheritance_chain: Vec<ChainNode>,
}

// GET /module-items/{id}/proctor-config
pub async fn get_item_config(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<ItemProctorConfigResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let rows = chain_rows(pool, item_id).await?;
    let Some(first) = rows.first() else { return Err(AppError::NotFound("module_item_not_found")) };
    let raw = first.3.clone();
    let max_depth = rows.last().map(|r| r.4).unwrap_or(0);
    let mut chain = Vec::new();
    if let Some(module) = module_of(pool, item_id).await? {
        chain.push(ChainNode {
            id: module.id,
            title: module.title,
            node_type: "module".to_string(),
            depth_from_leaf: max_depth + 1,
            has_own_config: usable(&module.proctor_config).is_some(),
            config: module.proctor_config,
        });
    }
    for (id, title, node_type, cfg, depth) in rows.into_iter().rev() {
        chain.push(ChainNode { id, title, node_type, depth_from_leaf: depth, has_own_config: usable(&cfg).is_some(), config: cfg });
    }
    Ok(ItemProctorConfigResponse { item_id, raw, effective: effective_for_item(pool, item_id).await?, inheritance_chain: chain })
}

/// `{}` and null both mean "inherit everything".
fn normalized(config: Option<Value>) -> Result<Option<Value>, AppError> {
    match config {
        Some(v) if v.as_object().is_some_and(|o| !o.is_empty()) => {
            validate_config(&v)?;
            Ok(Some(v))
        }
        Some(Value::Object(_)) | Some(Value::Null) | None => Ok(None),
        Some(_) => Err(invalid("config harus berupa objek")),
    }
}

// PUT /module-items/{id}/proctor-config — replaces the item's own
// overrides. A setting, not content, so it stays editable after publish.
pub async fn set_item_config(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, config: Option<Value>) -> Result<ItemProctorConfigResponse, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let config = normalized(config)?;
    let updated = sqlx::query!(r#"update module_items set proctor_config = $2, updated_at = now() where id = $1"#, item_id, config).execute(pool).await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound("module_item_not_found"));
    }
    get_item_config(pool, ctx, item_id).await
}

// GET /modules/{id}/proctor-config — the top of the cascade.
pub async fn get_module_config(pool: &PgPool, ctx: &AuthContext, module_id: Uuid) -> Result<Value, AppError> {
    require_permission(ctx, Resource::ModuleItem, Action::Create)?;
    let cfg = sqlx::query_scalar!(r#"select proctor_config from modules where id = $1"#, module_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("module_not_found"))?;
    Ok(json!({ "module_id": module_id, "raw": cfg }))
}

pub async fn set_module_config(pool: &PgPool, ctx: &AuthContext, module_id: Uuid, config: Option<Value>) -> Result<Value, AppError> {
    require_permission(ctx, Resource::Module, Action::Create)?;
    let config = normalized(config)?;
    let updated = sqlx::query!(r#"update modules set proctor_config = $2, updated_at = now() where id = $1"#, module_id, config).execute(pool).await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound("module_not_found"));
    }
    get_module_config(pool, ctx, module_id).await
}

// ── Geofence ────────────────────────────────────────────────────────

pub fn haversine_m(lat1: f64, lng1: f64, lat2: f64, lng2: f64) -> f64 {
    let rad = std::f64::consts::PI / 180.0;
    let d_phi = (lat2 - lat1) * rad;
    let d_lambda = (lng2 - lng1) * rad;
    let a = (d_phi / 2.0).sin().powi(2) + (lat1 * rad).cos() * (lat2 * rad).cos() * (d_lambda / 2.0).sin().powi(2);
    EARTH_RADIUS_M * 2.0 * a.sqrt().asin()
}

pub struct LocationCheck {
    pub within: bool,
    pub nearest_m: f64,
    pub nearest_label: Option<String>,
}

/// None when the geofence is off or has no points yet (the builder says
/// so: "Tambah minimal 1 supaya geofence bisa berjalan"). The reported
/// GPS accuracy (capped at 100 m) widens the radius, so a reading near
/// the edge is not rejected for the phone's own uncertainty.
pub fn check_location(config: &Value, lat: f64, lng: f64, accuracy_m: Option<f64>) -> Option<LocationCheck> {
    let geo = config.get("geofence")?;
    if !flag(geo, "enabled") {
        return None;
    }
    let centers = geo.get("centers").and_then(Value::as_array).filter(|c| !c.is_empty())?;
    let slack = accuracy_m.unwrap_or(0.0).clamp(0.0, 100.0);
    let mut best = LocationCheck { within: false, nearest_m: f64::MAX, nearest_label: None };
    for c in centers {
        let (Some(clat), Some(clng), Some(radius)) = (c.get("lat").and_then(Value::as_f64), c.get("lng").and_then(Value::as_f64), c.get("radius_m").and_then(Value::as_f64)) else {
            continue;
        };
        let distance = haversine_m(lat, lng, clat, clng);
        if distance <= radius + slack {
            best.within = true;
        }
        if distance < best.nearest_m {
            best.nearest_m = distance;
            best.nearest_label = c.get("label").and_then(Value::as_str).map(str::to_string);
        }
    }
    Some(best)
}

// ── Sittings ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StartSessionRequest {
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub accuracy_m: Option<f64>,
    pub id_card_asset_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct SessionState {
    /// None when the quiz is not proctored at all.
    pub session_id: Option<Uuid>,
    pub config: Value,
    pub locked: bool,
    pub lock_reason: Option<String>,
}

// POST /module-items/{id}/proctor-sessions — the learner's preflight
// passed; start (or refuse) a proctored sitting.
pub async fn start_session(pool: &PgPool, ctx: &AuthContext, item_id: Uuid, req: StartSessionRequest) -> Result<SessionState, AppError> {
    // Same visibility and access-gate rules as opening the item.
    module_item::get_detail(pool, ctx, item_id).await?;
    let config = effective_for_item(pool, item_id).await?;
    if !flag(&config, "enabled") {
        return Ok(SessionState { session_id: None, config, locked: false, lock_reason: None });
    }

    // A locked sitting stays locked until a tutor unlocks it — opening the
    // quiz again is not a way around the lock.
    let locked = sqlx::query!(
        r#"select id, lock_reason from quiz_proctor_sessions
           where item_id = $1 and user_id = $2 and attempt_id is null and locked_at is not null and started_at > now() - interval '1 day'
           order by started_at desc limit 1"#,
        item_id,
        ctx.user_id,
    )
    .fetch_optional(pool)
    .await?;
    if let Some(row) = locked {
        return Ok(SessionState { session_id: Some(row.id), config, locked: true, lock_reason: row.lock_reason });
    }

    if flag(&config, "require_id_card") && req.id_card_asset_id.is_none() {
        return Err(AppError::UnprocessableEntity("id_card_required", "Foto kartu identitas wajib diunggah sebelum mulai.".to_string()));
    }
    if config.get("geofence").is_some_and(|g| flag(g, "enabled")) {
        let (Some(lat), Some(lng)) = (req.lat, req.lng) else {
            return Err(AppError::UnprocessableEntity("location_required", "Kuis ini hanya bisa dikerjakan dari lokasi tertentu — izinkan akses lokasi dulu.".to_string()));
        };
        if let Some(check) = check_location(&config, lat, lng, req.accuracy_m) {
            if !check.within {
                return Err(AppError::UnprocessableEntity(
                    "outside_geofence",
                    format!(
                        "Kamu berada {:.0} m dari lokasi terdekat yang diizinkan{}. Kerjakan kuis ini dari lokasi yang ditentukan.",
                        check.nearest_m,
                        check.nearest_label.map(|l| format!(" ({l})")).unwrap_or_default()
                    ),
                ));
            }
        }
    }

    let session_id = sqlx::query_scalar!(
        r#"insert into quiz_proctor_sessions (user_id, item_id, config, id_card_asset_id) values ($1, $2, $3, $4) returning id"#,
        ctx.user_id,
        item_id,
        config,
        req.id_card_asset_id,
    )
    .fetch_one(pool)
    .await?;
    if let Some(asset_id) = req.id_card_asset_id {
        sqlx::query!(r#"insert into quiz_proctor_events (session_id, type, asset_id) values ($1, 'id_card', $2)"#, session_id, asset_id).execute(pool).await?;
    }
    Ok(SessionState { session_id: Some(session_id), config, locked: false, lock_reason: None })
}

#[derive(Debug, Deserialize)]
pub struct EventRequest {
    #[serde(rename = "type")]
    pub kind: String,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub accuracy_m: Option<f64>,
    pub asset_id: Option<Uuid>,
}

/// The reason to lock, if any auto-lock rule (or the geofence's
/// violation limit) has now been reached. Pure, for tests.
pub fn lock_reason_for(config: &Value, counts: &std::collections::HashMap<String, i64>) -> Option<String> {
    let count = |kind: &str| counts.get(kind).copied().unwrap_or(0);
    for rule in config.get("auto_lock_rules").and_then(Value::as_array).into_iter().flatten() {
        let kind = rule.get("type").and_then(Value::as_str).unwrap_or_default();
        let after = rule.get("after_count").and_then(Value::as_i64).unwrap_or(i64::MAX);
        if count(kind) >= after {
            let label = LOCKABLE.iter().find(|(k, _)| *k == kind).map(|(_, l)| *l).unwrap_or(kind);
            return Some(format!("{label} {}x", count(kind)));
        }
    }
    let geo = config.get("geofence")?;
    if flag(geo, "enabled") {
        if let Some(max) = geo.get("max_violations").and_then(Value::as_i64) {
            if count("location_violation") >= max {
                return Some(format!("Keluar dari lokasi yang diizinkan {}x", count("location_violation")));
            }
        }
    }
    None
}

// POST /quiz-proctor-sessions/{id}/events
pub async fn record_event(pool: &PgPool, ctx: &AuthContext, session_id: Uuid, req: EventRequest) -> Result<SessionState, AppError> {
    let session = sqlx::query!(
        r#"select user_id, config, locked_at, lock_reason, attempt_id from quiz_proctor_sessions where id = $1"#,
        session_id,
    )
    .fetch_optional(pool)
    .await?
    .filter(|s| s.user_id == ctx.user_id)
    .ok_or(AppError::NotFound("proctor_session_not_found"))?;
    let state = |locked: bool, reason: Option<String>| SessionState { session_id: Some(session_id), config: session.config.clone(), locked, lock_reason: reason };
    if session.locked_at.is_some() {
        return Ok(state(true, session.lock_reason.clone()));
    }
    if session.attempt_id.is_some() {
        return Err(AppError::UnprocessableEntity("session_closed", "sesi pengawasan ini sudah selesai".to_string()));
    }
    if !EVENT_TYPES.contains(&req.kind.as_str()) {
        return Err(AppError::UnprocessableEntity("invalid_event_type", format!("jenis kejadian \"{}\" tidak dikenal", req.kind)));
    }

    let (kind, detail) = if req.kind == "location" {
        let (Some(lat), Some(lng)) = (req.lat, req.lng) else {
            return Err(AppError::UnprocessableEntity("location_required", "lat dan lng wajib diisi".to_string()));
        };
        match check_location(&session.config, lat, lng, req.accuracy_m) {
            Some(check) if !check.within => ("location_violation".to_string(), Some(json!({ "lat": lat, "lng": lng, "distance_m": check.nearest_m.round() }))),
            // Inside the zone (or no zone): nothing worth storing.
            _ => {
                sqlx::query!(r#"update quiz_proctor_sessions set last_event_at = now() where id = $1"#, session_id).execute(pool).await?;
                return Ok(state(false, None));
            }
        }
    } else {
        (req.kind.clone(), None)
    };

    sqlx::query!(
        r#"insert into quiz_proctor_events (session_id, type, detail, asset_id) values ($1, $2, $3, $4)"#,
        session_id,
        kind,
        detail,
        req.asset_id,
    )
    .execute(pool)
    .await?;

    let counts: std::collections::HashMap<String, i64> = sqlx::query!(
        r#"select type, count(*) as "count!" from quiz_proctor_events where session_id = $1 group by type"#,
        session_id,
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| (r.r#type, r.count))
    .collect();

    match lock_reason_for(&session.config, &counts) {
        Some(reason) => {
            sqlx::query!(
                r#"update quiz_proctor_sessions set locked_at = now(), lock_reason = $2, last_event_at = now() where id = $1"#,
                session_id,
                reason,
            )
            .execute(pool)
            .await?;
            Ok(state(true, Some(reason)))
        }
        None => {
            sqlx::query!(r#"update quiz_proctor_sessions set last_event_at = now() where id = $1"#, session_id).execute(pool).await?;
            Ok(state(false, None))
        }
    }
}

// POST /quiz-proctor-sessions/{id}/unlock — a tutor lets the learner continue.
pub async fn unlock(pool: &PgPool, ctx: &AuthContext, session_id: Uuid) -> Result<(), AppError> {
    let item_id = sqlx::query_scalar!(r#"select item_id from quiz_proctor_sessions where id = $1"#, session_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("proctor_session_not_found"))?;
    if !item_progress::can_review_item(pool, ctx, item_id).await? {
        return Err(AppError::Forbidden);
    }
    // Counters restart after an unlock; the history stays in the events.
    let mut tx = pool.begin().await?;
    sqlx::query!(r#"update quiz_proctor_sessions set locked_at = null, lock_reason = null, unlocked_by = $2 where id = $1"#, session_id, ctx.user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(r#"update quiz_proctor_events set type = type || ':before_unlock' where session_id = $1 and type not like '%:before_unlock' and type not in ('snapshot', 'id_card')"#, session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_event_at: Option<chrono::DateTime<chrono::Utc>>,
    pub locked_at: Option<chrono::DateTime<chrono::Utc>>,
    pub lock_reason: Option<String>,
    pub submitted: bool,
    pub event_counts: Value,
}

// GET /module-items/{id}/proctor-sessions — the tutor's monitor.
pub async fn list_sessions(pool: &PgPool, ctx: &AuthContext, item_id: Uuid) -> Result<Vec<SessionSummary>, AppError> {
    if !item_progress::can_review_item(pool, ctx, item_id).await? {
        return Err(AppError::Forbidden);
    }
    let rows = sqlx::query_as!(
        SessionSummary,
        r#"select s.id, s.user_id, u.name, s.started_at, s.last_event_at, s.locked_at, s.lock_reason,
                  (s.attempt_id is not null) as "submitted!",
                  coalesce((select jsonb_object_agg(x.type, x.n) from (select type, count(*) as n from quiz_proctor_events e where e.session_id = s.id group by type) x), '{}'::jsonb) as "event_counts!"
           from quiz_proctor_sessions s join users u on u.id = s.user_id
           where s.item_id = $1
           order by s.started_at desc limit 200"#,
        item_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[derive(Debug, Serialize)]
pub struct SessionEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub detail: Option<Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Snapshots and the ID card: a short-lived signed URL. The photos are
    /// the learner's own uploads, so the tutor reaches them through this
    /// check rather than through Drive sharing.
    pub image_url: Option<String>,
}

// GET /quiz-proctor-sessions/{id}/events
pub async fn session_events(pool: &PgPool, config: &Config, storage: &dyn AssetStorage, ctx: &AuthContext, session_id: Uuid) -> Result<Vec<SessionEvent>, AppError> {
    let item_id = sqlx::query_scalar!(r#"select item_id from quiz_proctor_sessions where id = $1"#, session_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("proctor_session_not_found"))?;
    if !item_progress::can_review_item(pool, ctx, item_id).await? {
        return Err(AppError::Forbidden);
    }
    let rows = sqlx::query!(
        r#"select type, detail, asset_id, created_at from quiz_proctor_events where session_id = $1 order by created_at asc limit 1000"#,
        session_id,
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let image_url = match r.asset_id {
            Some(id) => storage.signed_url(&id.to_string(), config.asset_signed_url_ttl_seconds as u64).await.ok(),
            None => None,
        };
        out.push(SessionEvent { kind: r.r#type, detail: r.detail, created_at: r.created_at, image_url });
    }
    Ok(out)
}

/// For a proctored quiz, the sitting a submission belongs to — refusing
/// a submission that never went through the preflight (straight to the
/// API), since that is exactly what proctoring exists to prevent.
pub async fn session_for_submit(pool: &PgPool, user_id: Uuid, item_id: Uuid) -> Result<Option<Uuid>, AppError> {
    if !flag(&effective_for_item(pool, item_id).await?, "enabled") {
        return Ok(None);
    }
    let session = sqlx::query_scalar!(
        r#"select id from quiz_proctor_sessions
           where item_id = $1 and user_id = $2 and attempt_id is null and started_at > now() - interval '1 day'
           order by started_at desc limit 1"#,
        item_id,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    session.map(Some).ok_or_else(|| {
        AppError::UnprocessableEntity("proctor_session_required", "Kuis ini diawasi — buka lewat halaman kuis supaya pengawasan berjalan.".to_string())
    })
}

pub async fn close_session(pool: &PgPool, session_id: Uuid, attempt_id: Uuid) -> Result<(), AppError> {
    sqlx::query!(r#"update quiz_proctor_sessions set attempt_id = $2 where id = $1"#, session_id, attempt_id).execute(pool).await?;
    sqlx::query!(r#"insert into quiz_proctor_events (session_id, type) values ($1, 'submitted')"#, session_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn leaf_wins_and_null_clears() {
        let mut base = json!({ "enabled": true, "block_paste": true, "geofence": { "enabled": true, "max_violations": 3 } });
        deep_merge(&mut base, &json!({ "block_paste": false, "geofence": { "max_violations": 5 } }));
        assert_eq!(base, json!({ "enabled": true, "block_paste": false, "geofence": { "enabled": true, "max_violations": 5 } }));
        deep_merge(&mut base, &json!({ "geofence": null }));
        assert!(base.get("geofence").is_none());
    }

    #[test]
    fn validation() {
        assert!(validate_config(&json!({ "enabled": true, "snapshot_interval_sec": 30, "auto_lock_rules": [{ "type": "tab_hidden", "after_count": 3 }] })).is_ok());
        assert!(validate_config(&json!({ "snapshot_interval_sec": 1 })).is_err());
        assert!(validate_config(&json!({ "auto_lock_rules": [{ "type": "sneeze", "after_count": 1 }] })).is_err());
        assert!(validate_config(&json!({ "geofence": { "centers": [{ "lat": 200, "lng": 0, "radius_m": 50 }] } })).is_err());
        assert!(validate_config(&json!({ "mystery": true })).is_err());
    }

    #[test]
    fn geofence_with_accuracy_slack() {
        // Two points ~111 m apart (0.001° of latitude).
        let config = json!({ "geofence": { "enabled": true, "centers": [{ "label": "Sekolah", "lat": -7.8, "lng": 112.0, "radius_m": 100 }] } });
        let edge = check_location(&config, -7.801, 112.0, Some(0.0)).unwrap();
        assert!(!edge.within);
        assert_eq!(edge.nearest_label.as_deref(), Some("Sekolah"));
        assert!(check_location(&config, -7.801, 112.0, Some(20.0)).unwrap().within);
        assert!(check_location(&json!({ "geofence": { "enabled": true, "centers": [] } }), 0.0, 0.0, None).is_none());
    }

    #[test]
    fn auto_lock_rules_and_violation_limit() {
        let config = json!({ "auto_lock_rules": [{ "type": "fullscreen_exit", "after_count": 2 }], "geofence": { "enabled": true, "max_violations": 3 } });
        let mut counts = HashMap::from([("fullscreen_exit".to_string(), 1)]);
        assert!(lock_reason_for(&config, &counts).is_none());
        counts.insert("fullscreen_exit".into(), 2);
        assert_eq!(lock_reason_for(&config, &counts).as_deref(), Some("Keluar dari layar penuh 2x"));
        let geo = HashMap::from([("location_violation".to_string(), 3)]);
        assert!(lock_reason_for(&config, &geo).unwrap().contains("lokasi"));
    }
}
