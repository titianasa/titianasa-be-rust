-- P40-004 (ADR-0014 §3) — daily aggregate tables. Admin Pusat's
-- dashboard (P40-005) never runs an analytic query against a
-- transaction table at page-load time; it reads these instead, kept
-- fresh by the `metrics_rollup` job (P40-002's queue, registered in
-- `services/metrics_rollup.rs`).
--
-- `day` is a WIB (Asia/Jakarta) calendar date everywhere — computed as
-- `(created_at at time zone 'Asia/Jakarta')::date` at rollup time, never
-- the UTC date, so "today" in the dashboard matches "today" for a user
-- in Jakarta.
--
-- Every table's primary key is exactly its grouping dimensions, so the
-- rollup's write is `insert ... on conflict (...) do update` — running
-- it twice for the same day only ever overwrites with the same freshly
-- recomputed truth, never adds a duplicate row. A dimension that can be
-- legitimately absent (e.g. no `subscription_tier` on an `enrollment`
-- order) is COALESCEd to a fixed sentinel at write time rather than
-- left NULL — Postgres treats NULL as never equal to NULL, so a NULL
-- column inside a PRIMARY KEY would silently defeat the whole
-- upsert-not-duplicate guarantee.

create table metrics_daily_sales (
  day date not null,
  kind text not null,
  -- 'none' sentinel for kind='enrollment' (no subscription_tier) — see
  -- file header on why this can't be a bare NULL in a PK.
  subscription_tier text not null default 'none',
  orders integer not null default 0,
  paid_orders integer not null default 0,
  revenue_idr bigint not null default 0,
  primary key (day, kind, subscription_tier)
);

create table metrics_daily_subscriptions (
  day date not null,
  tier text not null,
  active integer not null default 0,
  new integer not null default 0,
  churned integer not null default 0,
  primary key (day, tier)
);

create table metrics_daily_users (
  day date not null,
  -- Nil-UUID sentinel for "no organization" (individual self-learner,
  -- the common case) — same NULL-in-a-PK reasoning as above.
  org_id uuid not null default '00000000-0000-0000-0000-000000000000',
  -- 'belum_diisi' sentinel for a user who signed up but hasn't
  -- completed onboarding (user_learning_profiles.jenjang is NULL) yet.
  jenjang text not null default 'belum_diisi',
  signups integer not null default 0,
  primary key (day, org_id, jenjang)
);

-- Deliberately NOT backfilled per historical day (see
-- services/metrics_rollup.rs's module doc) — curriculum coverage as it
-- looked on a past day isn't reconstructable without a change log this
-- codebase doesn't keep. Every rollup run writes/overwrites only
-- TODAY's row; this table is a running daily snapshot from the day the
-- job first ran, not a full history.
create table metrics_daily_content (
  day date not null,
  subject_id uuid not null,
  tahap integer not null,
  topics_total integer not null default 0,
  topics_with_module integer not null default 0,
  topics_with_quiz integer not null default 0,
  questions_total integer not null default 0,
  questions_by_bloom jsonb not null default '{}'::jsonb,
  questions_by_difficulty jsonb not null default '{}'::jsonb,
  primary key (day, subject_id, tahap)
);

create table metrics_daily_ai (
  day date not null,
  task_type text not null,
  model text not null,
  calls integer not null default 0,
  failed integer not null default 0,
  tokens bigint not null default 0,
  -- Estimated from `ai_model_catalog.price_output_per_mtok_idr`
  -- (P40-003) — 0 until an admin fills in a price for that model, then
  -- accurate on the next rollup with no code change needed.
  cost_idr bigint not null default 0,
  primary key (day, task_type, model)
);
