-- The module builder's per-item rules, ported from parelabs:
--
--   guard_config      "Aturan Akses & Guard" — whether the item is required,
--                     and whether passing it gates the next section/module.
--   attendance_guard  "Attendance Guard" — a class member cannot check in to
--                     the next class session until this item (or its whole
--                     folder) is done.
--   proctor_config    "Keamanan Ujian (Proctor)" — on items AND folders AND
--                     modules; absent keys inherit from above, so a rule set
--                     on a folder covers everything inside it.
alter table module_items add column guard_config jsonb;
alter table module_items add column attendance_guard jsonb;
alter table module_items add column proctor_config jsonb;
alter table modules add column proctor_config jsonb;

-- What every gate above is evaluated against: whether a learner has
-- finished an item, their best score on it, and a tutor's approval.
create table module_item_progress (
  user_id uuid not null references users(id) on delete cascade,
  item_id uuid not null references module_items(id) on delete cascade,
  completed_at timestamptz,
  best_score double precision,
  approved_at timestamptz,
  approved_by uuid references users(id) on delete set null,
  updated_at timestamptz not null default now(),
  primary key (user_id, item_id)
);
create index module_item_progress_item_idx on module_item_progress (item_id);

-- One proctored sitting of a quiz item. A module quiz only creates its
-- attempt row at submit time, so the sitting is tracked on its own and
-- linked to the attempt once it is submitted. `config` is the effective
-- proctor config frozen at start, so an edit mid-exam never changes the
-- rules a running exam is held to.
create table quiz_proctor_sessions (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users(id) on delete cascade,
  item_id uuid not null references module_items(id) on delete cascade,
  config jsonb not null,
  id_card_asset_id uuid,
  started_at timestamptz not null default now(),
  last_event_at timestamptz,
  locked_at timestamptz,
  lock_reason text,
  unlocked_by uuid references users(id) on delete set null,
  attempt_id uuid
);
create index quiz_proctor_sessions_lookup_idx on quiz_proctor_sessions (item_id, user_id, started_at desc);

create table quiz_proctor_events (
  id uuid primary key default gen_random_uuid(),
  session_id uuid not null references quiz_proctor_sessions(id) on delete cascade,
  type text not null,
  detail jsonb,
  asset_id uuid,
  created_at timestamptz not null default now()
);
create index quiz_proctor_events_session_idx on quiz_proctor_events (session_id, created_at);
