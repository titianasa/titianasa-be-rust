-- Admin Pusat "Peserta" dashboard (ADR-0014 §2, the piece that shipped
-- after Fase 39's learning_events pipeline landed) plus new tracking
-- infrastructure ADR-0014 never speced: "sedang online sekarang" and a
-- per-participant detail view (IP/device/region/login history). `users`
-- has never had activity/login columns, so this is genuinely new state,
-- not just a new read of existing tables.

-- One row per successful Google OAuth login — login history and "last
-- login" for the participant detail page. `ip` is plain text, not
-- `inet` — this app never does CIDR arithmetic on it, only displays it
-- and hands it to a geolocation lookup, and `inet` would need sqlx's
-- optional `ipnetwork` feature (a new dependency) for no real benefit.
create table user_login_events (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users(id) on delete cascade,
  occurred_at timestamptz not null default now(),
  ip text,
  user_agent text,
  device_label text
);

create index user_login_events_user_idx on user_login_events (user_id, occurred_at desc);

-- One row per user, upserted on every authenticated request (throttled
-- — see middleware/auth.rs). last_seen_at is both "sedang online" (< 5
-- minutes old) and "terakhir aktif" for everyone else. region is filled
-- LAZILY: only when an admin opens that participant's detail page (a
-- live reqwest lookup against a third-party IP geolocation API), so a
-- plain login/request never makes an outbound call. Nullable because
-- most rows never get a region lookup at all.
create table user_presence (
  user_id uuid primary key references users(id) on delete cascade,
  last_seen_at timestamptz not null default now(),
  last_ip text,
  last_user_agent text,
  last_device_label text,
  region text,
  region_resolved_at timestamptz
);

create index user_presence_last_seen_idx on user_presence (last_seen_at);

-- DAU/WAU/MAU trend, same shape/rollup convention as metrics_daily_*
-- (P40-004): one row per WIB day, upserted by the same metrics_rollup
-- job, recomputed for the last 3 days each run for late-arriving events.
create table metrics_daily_participants (
  day date primary key,
  dau integer not null default 0,
  wau integer not null default 0,
  mau integer not null default 0
);

-- Weekly retention cohorts: of everyone who signed up in `cohort_week`,
-- how many were still active (any learning_events row) `weeks_since_signup`
-- weeks later. cohort_size is frozen once the cohort week is fully in
-- the past (see metrics_rollup.rs); active_users keeps being
-- recomputed for the trailing 12 weeks each run, same late-arriving-
-- event reasoning as the daily tables.
create table metrics_weekly_retention (
  cohort_week date not null,
  weeks_since_signup integer not null,
  cohort_size integer not null default 0,
  active_users integer not null default 0,
  primary key (cohort_week, weeks_since_signup)
);
