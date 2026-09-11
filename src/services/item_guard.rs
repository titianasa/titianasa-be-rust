// "Aturan Akses & Guard" and "Attendance Guard", ported from parelabs'
// module builder.
//
// Unlike parelabs, where the e-learning sidebar decides locks in the
// browser, locks here are decided on the server: the learner's item tree
// carries them, and opening a locked item by URL is refused. A lock that
// only the UI enforces is a suggestion.
//
// guard_config (on an item):
//   completion_rule  "optional" | "required"
//       required — later items in the SAME folder stay locked until this
//       one is finished.
//   gate_rule        "none" | "section_gate" | "module_gate"
//       section_gate — items in LATER top-level folders stay locked until
//       this item meets its condition; module_gate — every later item.
//   gate_condition   { type: "submit" | "pass" | "tutor_approve", min_score_pct }
//
// attendance_guard (on an item):
//   { type: "this_item_complete" | "section_complete", message }
//   A class member cannot check in to a session of a class that studies
//   this module until the item (or every item in its folder) is finished.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;
use crate::services::item_progress::{self, Progress};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GateCondition {
    #[serde(rename = "type", default = "default_condition")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_score_pct: Option<f64>,
}

fn default_condition() -> String {
    "submit".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GuardConfig {
    #[serde(default)]
    pub completion_rule: Option<String>,
    #[serde(default)]
    pub gate_rule: Option<String>,
    #[serde(default)]
    pub gate_condition: Option<GateCondition>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AttendanceGuard {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn invalid(detail: &str) -> AppError {
    AppError::UnprocessableEntity("invalid_guard", detail.to_string())
}

/// Parses and checks a guard_config; `None`/`{}` mean "no rules".
pub fn validate_guard(value: Option<&Value>) -> Result<Option<GuardConfig>, AppError> {
    let Some(value) = value.filter(|v| !v.is_null() && v.as_object().is_some_and(|o| !o.is_empty())) else { return Ok(None) };
    let guard: GuardConfig = serde_json::from_value(value.clone()).map_err(|e| invalid(&format!("guard_config tidak sesuai bentuknya: {e}")))?;
    if let Some(rule) = guard.completion_rule.as_deref() {
        if !["optional", "required"].contains(&rule) {
            return Err(invalid("completion_rule harus optional atau required"));
        }
    }
    if let Some(rule) = guard.gate_rule.as_deref() {
        if !["none", "section_gate", "module_gate"].contains(&rule) {
            return Err(invalid("gate_rule harus none, section_gate, atau module_gate"));
        }
    }
    if let Some(cond) = &guard.gate_condition {
        if !["submit", "pass", "tutor_approve"].contains(&cond.kind.as_str()) {
            return Err(invalid("gate_condition.type harus submit, pass, atau tutor_approve"));
        }
        if let Some(pct) = cond.min_score_pct {
            if !(0.0..=100.0).contains(&pct) {
                return Err(invalid("min_score_pct harus 0-100"));
            }
        }
    }
    Ok(Some(guard))
}

pub fn validate_attendance_guard(value: Option<&Value>) -> Result<Option<AttendanceGuard>, AppError> {
    let Some(value) = value.filter(|v| !v.is_null()) else { return Ok(None) };
    let guard: AttendanceGuard = serde_json::from_value(value.clone()).map_err(|e| invalid(&format!("attendance_guard tidak sesuai bentuknya: {e}")))?;
    match guard.kind.as_str() {
        "none" => Ok(None),
        "this_item_complete" | "section_complete" => Ok(Some(guard)),
        _ => Err(invalid("attendance_guard.type harus this_item_complete atau section_complete")),
    }
}

/// One node of a module's item tree, flattened in reading order.
#[derive(Debug, Clone)]
pub struct TreeItem {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub is_section: bool,
    pub title: String,
    pub published: bool,
    pub guard: Option<GuardConfig>,
}

/// Whether `progress` meets a gate's condition.
fn gate_met(progress: Option<&Progress>, cond: Option<&GateCondition>) -> bool {
    let Some(p) = progress else { return false };
    match cond.map(|c| c.kind.as_str()).unwrap_or("submit") {
        "pass" => {
            let min = cond.and_then(|c| c.min_score_pct).unwrap_or(70.0);
            p.completed && p.best_score.is_some_and(|s| s >= min)
        }
        "tutor_approve" => p.approved,
        _ => p.completed,
    }
}

fn condition_label(cond: Option<&GateCondition>) -> String {
    match cond.map(|c| c.kind.as_str()).unwrap_or("submit") {
        "pass" => format!("mencapai skor {}%", cond.and_then(|c| c.min_score_pct).unwrap_or(70.0)),
        "tutor_approve" => "dinilai tutor".to_string(),
        _ => "diselesaikan".to_string(),
    }
}

/// The root-level folder an item sits in (None at the module's root).
fn top_section(id: Uuid, parents: &HashMap<Uuid, Option<Uuid>>) -> Option<Uuid> {
    let mut current = parents.get(&id).copied().flatten()?;
    for _ in 0..10 {
        match parents.get(&current).copied().flatten() {
            Some(parent) => current = parent,
            None => return Some(current),
        }
    }
    Some(current)
}

/// item id → why it is locked, for every locked item. Pure, so the rules
/// can be tested without a database.
pub fn compute_locks(items: &[TreeItem], progress: &HashMap<Uuid, Progress>) -> HashMap<Uuid, String> {
    let parents: HashMap<Uuid, Option<Uuid>> = items.iter().map(|i| (i.id, i.parent_id)).collect();
    // Only published items gate anything: an unpublished "required" item
    // would otherwise lock a module forever.
    let leaves: Vec<&TreeItem> = items.iter().filter(|i| !i.is_section).collect();
    let mut locks = HashMap::new();

    for (index, item) in leaves.iter().enumerate() {
        for prev in &leaves[..index] {
            let Some(guard) = prev.guard.as_ref().filter(|_| prev.published) else { continue };
            let p = progress.get(&prev.id);
            let cond = guard.gate_condition.as_ref();

            if guard.completion_rule.as_deref() == Some("required") && prev.parent_id == item.parent_id && !gate_met(p, None) {
                locks.insert(item.id, format!("Selesaikan \"{}\" terlebih dahulu", prev.title));
                break;
            }
            match guard.gate_rule.as_deref() {
                Some("section_gate") if top_section(prev.id, &parents) != top_section(item.id, &parents) && !gate_met(p, cond) => {
                    locks.insert(item.id, format!("\"{}\" harus {} untuk membuka bagian ini", prev.title, condition_label(cond)));
                    break;
                }
                Some("module_gate") if !gate_met(p, cond) => {
                    locks.insert(item.id, format!("\"{}\" harus {} untuk melanjutkan", prev.title, condition_label(cond)));
                    break;
                }
                _ => {}
            }
        }
    }
    locks
}

struct Row {
    id: Uuid,
    parent_id: Option<Uuid>,
    node_type: String,
    title: String,
    status: String,
    order_index: i32,
    guard_config: Option<Value>,
}

/// A module's items in reading order (depth-first by order_index).
pub async fn load_tree(pool: &PgPool, module_id: Uuid) -> Result<Vec<TreeItem>, AppError> {
    let rows = sqlx::query_as!(
        Row,
        r#"select id, parent_id, node_type, title, status, order_index, guard_config from module_items where module_id = $1"#,
        module_id,
    )
    .fetch_all(pool)
    .await?;
    let mut by_parent: HashMap<Option<Uuid>, Vec<Row>> = HashMap::new();
    for row in rows {
        by_parent.entry(row.parent_id).or_default().push(row);
    }
    for list in by_parent.values_mut() {
        list.sort_by_key(|r| r.order_index);
    }
    fn walk(parent: Option<Uuid>, by_parent: &mut HashMap<Option<Uuid>, Vec<Row>>, out: &mut Vec<TreeItem>) {
        let Some(rows) = by_parent.remove(&parent) else { return };
        for r in rows {
            let id = r.id;
            out.push(TreeItem {
                id,
                parent_id: r.parent_id,
                is_section: r.node_type == "section",
                title: r.title,
                published: r.status == "published",
                // A malformed stored guard is ignored rather than locking
                // learners out over a config typo.
                guard: validate_guard(r.guard_config.as_ref()).ok().flatten(),
            });
            walk(Some(id), by_parent, out);
        }
    }
    let mut out = Vec::new();
    walk(None, &mut by_parent, &mut out);
    Ok(out)
}

pub struct LearnerState {
    pub locks: HashMap<Uuid, String>,
    pub completed: HashSet<Uuid>,
}

pub async fn learner_state(pool: &PgPool, module_id: Uuid, user_id: Uuid) -> Result<LearnerState, AppError> {
    let items = load_tree(pool, module_id).await?;
    let ids: Vec<Uuid> = items.iter().map(|i| i.id).collect();
    let progress = item_progress::for_user(pool, user_id, &ids).await?;
    let completed = progress.iter().filter(|(_, p)| p.completed).map(|(id, _)| *id).collect();
    Ok(LearnerState { locks: compute_locks(&items, &progress), completed })
}

/// Why `item_id` is locked for this learner, if it is.
pub async fn lock_reason(pool: &PgPool, item_id: Uuid, user_id: Uuid) -> Result<Option<String>, AppError> {
    let Some(module_id) = sqlx::query_scalar!(r#"select module_id from module_items where id = $1"#, item_id).fetch_optional(pool).await? else {
        return Ok(None);
    };
    Ok(learner_state(pool, module_id, user_id).await?.locks.remove(&item_id))
}

#[derive(Debug, Clone, Serialize)]
pub struct GuardBlocker {
    pub module_item_id: Uuid,
    pub module_item_title: String,
    pub guard_type: String,
    pub message: Option<String>,
}

/// Every unmet attendance guard for `student_id` in the modules `class_id`
/// studies. Empty = free to check in.
pub async fn attendance_blockers(pool: &PgPool, student_id: Uuid, class_id: Uuid) -> Result<Vec<GuardBlocker>, AppError> {
    let class = sqlx::query!(r#"select module_id, program_id from classes where id = $1"#, class_id).fetch_optional(pool).await?;
    let Some(class) = class else { return Ok(Vec::new()) };
    let mut module_ids: Vec<Uuid> = class.module_id.into_iter().collect();
    if let Some(program_id) = class.program_id {
        module_ids.extend(sqlx::query_scalar!(r#"select module_id from program_modules where program_id = $1"#, program_id).fetch_all(pool).await?);
    }
    // A class may point at a reference row; its items live in the
    // library module it references.
    let mut resolved = Vec::new();
    for id in module_ids {
        resolved.push(crate::services::module::resolve_content_module(pool, id).await?);
    }
    resolved.sort();
    resolved.dedup();

    let guarded = sqlx::query!(
        r#"select id, parent_id, title, attendance_guard as "attendance_guard!" from module_items
           where module_id = any($1) and node_type = 'item' and status = 'published' and attendance_guard is not null"#,
        &resolved[..],
    )
    .fetch_all(pool)
    .await?;
    if guarded.is_empty() {
        return Ok(Vec::new());
    }

    let mut blockers = Vec::new();
    for g in guarded {
        let Some(guard) = validate_attendance_guard(Some(&g.attendance_guard)).ok().flatten() else { continue };
        let required: Vec<Uuid> = match (guard.kind.as_str(), g.parent_id) {
            ("section_complete", Some(parent)) => sqlx::query_scalar!(
                r#"select id from module_items where parent_id = $1 and node_type = 'item' and status = 'published'"#,
                parent,
            )
            .fetch_all(pool)
            .await?,
            _ => vec![g.id],
        };
        let progress = item_progress::for_user(pool, student_id, &required).await?;
        let done = required.iter().all(|id| progress.get(id).is_some_and(|p| p.completed));
        if !done {
            blockers.push(GuardBlocker { module_item_id: g.id, module_item_title: g.title, guard_type: guard.kind, message: guard.message });
        }
    }
    Ok(blockers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn item(n: u128, parent: Option<u128>, guard: Option<GuardConfig>) -> TreeItem {
        TreeItem { id: id(n), parent_id: parent.map(id), is_section: false, title: format!("Item {n}"), published: true, guard }
    }

    fn section(n: u128) -> TreeItem {
        TreeItem { id: id(n), parent_id: None, is_section: true, title: format!("Bab {n}"), published: true, guard: None }
    }

    fn gate(rule: &str, kind: &str, min: Option<f64>) -> Option<GuardConfig> {
        Some(GuardConfig { completion_rule: None, gate_rule: Some(rule.into()), gate_condition: Some(GateCondition { kind: kind.into(), min_score_pct: min }) })
    }

    #[test]
    fn required_item_locks_later_siblings_only() {
        let required = Some(GuardConfig { completion_rule: Some("required".into()), ..Default::default() });
        let items = vec![section(1), item(2, Some(1), required), item(3, Some(1), None), section(4), item(5, Some(4), None)];
        let locks = compute_locks(&items, &HashMap::new());
        assert!(locks.contains_key(&id(3)));
        assert!(!locks.contains_key(&id(5)), "a different folder is not held by a required item");
        let done = HashMap::from([(id(2), Progress { completed: true, ..Default::default() })]);
        assert!(compute_locks(&items, &done).is_empty());
    }

    #[test]
    fn section_gate_with_pass_condition() {
        let items = vec![section(1), item(2, Some(1), gate("section_gate", "pass", Some(80.0))), item(3, Some(1), None), section(4), item(5, Some(4), None)];
        let low = HashMap::from([(id(2), Progress { completed: true, best_score: Some(60.0), approved: false })]);
        let locks = compute_locks(&items, &low);
        assert!(!locks.contains_key(&id(3)), "same folder stays open");
        assert!(locks[&id(5)].contains("80%"));
        let high = HashMap::from([(id(2), Progress { completed: true, best_score: Some(85.0), approved: false })]);
        assert!(compute_locks(&items, &high).is_empty());
    }

    #[test]
    fn module_gate_waits_for_tutor() {
        let items = vec![item(1, None, gate("module_gate", "tutor_approve", None)), item(2, None, None)];
        let submitted = HashMap::from([(id(1), Progress { completed: true, best_score: Some(100.0), approved: false })]);
        assert!(compute_locks(&items, &submitted)[&id(2)].contains("dinilai tutor"));
        let approved = HashMap::from([(id(1), Progress { completed: true, best_score: None, approved: true })]);
        assert!(compute_locks(&items, &approved).is_empty());
    }

    #[test]
    fn unpublished_gates_nothing() {
        let mut gated = item(1, None, gate("module_gate", "submit", None));
        gated.published = false;
        assert!(compute_locks(&[gated, item(2, None, None)], &HashMap::new()).is_empty());
    }

    #[test]
    fn validation_rejects_unknown_values() {
        assert!(validate_guard(Some(&serde_json::json!({"gate_rule": "everything"}))).is_err());
        assert!(validate_guard(Some(&serde_json::json!({"gate_condition": {"type": "pass", "min_score_pct": 150}}))).is_err());
        assert_eq!(validate_guard(Some(&serde_json::json!({}))).unwrap(), None);
        assert_eq!(validate_attendance_guard(Some(&serde_json::json!({"type": "none"}))).unwrap(), None);
        assert!(validate_attendance_guard(Some(&serde_json::json!({"type": "soon"}))).is_err());
    }
}
