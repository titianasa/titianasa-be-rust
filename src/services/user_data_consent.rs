// P39-007 (ADR-0013 "Privasi") — consent for the two NEW data
// categories this engine introduces: `learning_analytics` (client
// telemetry, P39-005) and `ai_chat_storage` (Live AI Chat questions
// stored verbatim, P39-006). Every SERVER-authoritative event from
// P39-004 (question_answered, quiz_attempt_submitted,
// module_item_completed, question_graded, exam_session_*) stays
// unconditional — see `learning_event.rs`'s registry, where those all
// declare `requires_consent: None`.
//
// Titian's learners include SD/SMP-age children. ADR-0013's own
// privacy section (citing UU No. 27/2022) calls for guardian
// confirmation before a minor's `granted=true` is accepted — the CHECK
// lives here, at the one place consent gets written, rather than
// trusted to every future caller to remember. Whether it's actually
// ENFORCED is `Config::consent_guardian_confirmation_required`: off
// during the current uji coba (pilot) phase per explicit product
// instruction (2026-09-13 — "untuk anak, di uji coba allow aja"), so a
// minor can self-grant without a guardian step for now. Turning
// enforcement on for a real launch is a config flip, not new code.

use sqlx::PgPool;
use uuid::Uuid;

use crate::errors::AppError;

pub const CONSENT_KINDS: &[&str] = &["learning_analytics", "ai_chat_storage"];

fn is_known_kind(kind: &str) -> bool {
    CONSENT_KINDS.contains(&kind)
}

/// SD ("SD-1".."SD-6") and SMP ("SMP-7".."SMP-9") jenjang codes — see
/// `user_learning_profiles.jenjang` and the onboarding form
/// (`titian-web/src/app/mulai`) that writes these exact prefixes.
fn is_minor_jenjang(jenjang: &str) -> bool {
    jenjang.starts_with("SD") || jenjang.starts_with("SMP")
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsentState {
    pub kind: String,
    pub granted: bool,
    pub guardian_confirmed: bool,
    pub granted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Every known consent kind for `user_id`, defaulting a kind with no
/// row yet to "not granted" — the safe default (ADR-0013: no consent,
/// no telemetry) rather than requiring every reader to handle "missing"
/// as a third state alongside granted/revoked.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<ConsentState>, AppError> {
    let rows = sqlx::query!(r#"select kind, granted, guardian_confirmed, granted_at, revoked_at from user_data_consents where user_id = $1"#, user_id)
        .fetch_all(pool)
        .await?;
    let mut by_kind: std::collections::HashMap<String, ConsentState> = rows
        .into_iter()
        .map(|r| (r.kind.clone(), ConsentState { kind: r.kind, granted: r.granted, guardian_confirmed: r.guardian_confirmed, granted_at: r.granted_at, revoked_at: r.revoked_at }))
        .collect();
    Ok(CONSENT_KINDS
        .iter()
        .map(|&kind| by_kind.remove(kind).unwrap_or(ConsentState { kind: kind.to_string(), granted: false, guardian_confirmed: false, granted_at: None, revoked_at: None }))
        .collect())
}

/// Whether `user_id` currently has `kind` granted. `false` for an
/// unrecognised kind rather than erroring — a caller checking gate
/// status should never be blocked by a typo turning into a panic path;
/// `set_consent` is where an unknown kind is a real rejection.
pub async fn has_consent(pool: &PgPool, user_id: Uuid, kind: &str) -> Result<bool, AppError> {
    if !is_known_kind(kind) {
        return Ok(false);
    }
    let granted = sqlx::query_scalar!(r#"select granted from user_data_consents where user_id = $1 and kind = $2"#, user_id, kind).fetch_optional(pool).await?;
    Ok(granted.unwrap_or(false))
}

/// Grants or revokes one consent kind for `user_id`. `granted_by` is
/// who is performing the action (the user themself, or a guardian on a
/// minor's account — see the migration's own note on why that can
/// differ from `user_id`). `guardian_confirmed` must be `true` to grant
/// (not needed to revoke) whenever the user's own `jenjang` is SD/SMP —
/// checked fresh here, not trusted from the caller, so a minor's
/// consent can never be silently granted by a client that skipped the
/// guardian step.
pub async fn set_consent(
    pool: &PgPool,
    user_id: Uuid,
    kind: &str,
    granted: bool,
    granted_by: Uuid,
    guardian_confirmed: bool,
    require_guardian_confirmation: bool,
) -> Result<ConsentState, AppError> {
    if !is_known_kind(kind) {
        return Err(AppError::UnprocessableEntity("unknown_consent_kind", format!(r#"consent kind "{kind}" is not recognised"#)));
    }

    // `require_guardian_confirmation` is `Config::consent_guardian_confirmation_required`
    // — `false` during the current uji coba (pilot) phase per explicit
    // product instruction, `true` once this launches for real. The
    // CHECK itself doesn't change; only whether it's enforced does, so
    // turning enforcement on later needs no new code, only a config flip.
    if granted && require_guardian_confirmation {
        let jenjang = sqlx::query_scalar!(r#"select jenjang from user_learning_profiles where user_id = $1"#, user_id).fetch_optional(pool).await?.flatten();
        if jenjang.as_deref().is_some_and(is_minor_jenjang) && !guardian_confirmed {
            return Err(AppError::UnprocessableEntity("guardian_confirmation_required", "persetujuan untuk jenjang SD/SMP wajib dikonfirmasi orang tua/wali".to_string()));
        }
    }

    let row = sqlx::query!(
        r#"insert into user_data_consents (user_id, kind, granted, granted_by, guardian_confirmed, granted_at, revoked_at)
           values ($1, $2, $3, $4, $5, case when $3 then now() else null end, case when $3 then null else now() end)
           on conflict (user_id, kind) do update
              set granted = excluded.granted,
                  granted_by = excluded.granted_by,
                  -- Once confirmed by a guardian, a later REVOKE by the
                  -- same account shouldn't erase that history; only a
                  -- fresh grant re-states it.
                  guardian_confirmed = case when excluded.granted then excluded.guardian_confirmed else user_data_consents.guardian_confirmed end,
                  granted_at = case when excluded.granted then now() else user_data_consents.granted_at end,
                  revoked_at = case when excluded.granted then null else now() end,
                  updated_at = now()
           returning kind, granted, guardian_confirmed, granted_at, revoked_at"#,
        user_id,
        kind,
        granted,
        granted_by,
        guardian_confirmed,
    )
    .fetch_one(pool)
    .await?;

    Ok(ConsentState { kind: row.kind, granted: row.granted, guardian_confirmed: row.guardian_confirmed, granted_at: row.granted_at, revoked_at: row.revoked_at })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sd_and_smp_jenjang_codes_are_minors_everything_else_is_not() {
        for j in ["SD-1", "SD-6", "SMP-7", "SMP-9"] {
            assert!(is_minor_jenjang(j), "{j} should require guardian confirmation");
        }
        for j in ["SMA-10", "SMA-12", "S1", "S2"] {
            assert!(!is_minor_jenjang(j), "{j} should not");
        }
    }

    #[test]
    fn only_the_two_declared_kinds_are_known() {
        assert!(is_known_kind("learning_analytics"));
        assert!(is_known_kind("ai_chat_storage"));
        assert!(!is_known_kind("something_else"));
    }
}
