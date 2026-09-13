-- P39-007 (ADR-0013 "Privasi") — consent for the two data categories
-- ADR-0013 draws a line around: `learning_analytics` (client behaviour
-- telemetry — P39-005's `POST /events`) and `ai_chat_storage` (storing
-- the literal text of a student's question to the Live AI Chat tutor —
-- P39-006). Every SERVER-authoritative event added in P39-004
-- (question_answered, quiz_attempt_submitted, module_item_completed,
-- question_graded, exam_session_*) stays ungated by design — those are
-- "penilaian" (grading/progress), which ADR-0013 §1.4 explicitly calls
-- out as always recorded regardless of consent, the same as a purchase.
--
-- One row per (user, kind) — the CURRENT state, not a history log.
-- `granted`/`granted_at`/`revoked_at` together read as: granted=true
-- means currently opted in (granted_at is when), granted=false with a
-- revoked_at means "opted in once, then withdrew", granted=false with
-- no revoked_at means "never asked, or asked and declined".
create table user_data_consents (
  user_id uuid not null references users(id) on delete cascade,
  kind text not null check (kind in ('learning_analytics', 'ai_chat_storage')),
  granted boolean not null,
  -- Who actually clicked grant — the user themself, or (for a minor,
  -- see guardian_confirmed) a parent/guardian acting on their account.
  -- No separate guardian-account model exists yet, so this is the same
  -- as user_id in practice today; the column exists so that doesn't
  -- have to be assumed by every reader.
  granted_by uuid references users(id) on delete set null,
  -- Required true before `granted=true` is accepted at all for a
  -- learner whose user_learning_profiles.jenjang is SD/SMP — enforced
  -- in services/user_data_consent.rs::set_consent, not here (a CHECK
  -- referencing another table isn't expressible as a constraint).
  guardian_confirmed boolean not null default false,
  granted_at timestamptz,
  revoked_at timestamptz,
  updated_at timestamptz not null default now(),
  primary key (user_id, kind)
);
