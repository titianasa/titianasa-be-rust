-- P39-003 (ADR-0013 L1) — learning_events grows far faster than every
-- other table (one row per graded answer today; per-section reads and
-- client telemetry are added in P39-004/005/006), so it needs monthly
-- partitioning from the start rather than retrofitting it once it's
-- millions of rows. Postgres can't convert an existing table to
-- partitioned in place, so this renames the old table, creates a new
-- partitioned one with the same name, copies the 16 existing rows over,
-- and drops the old one.
--
-- Every new column is ADDITIVE — mastery.rs/frss.rs only ever read
-- event_type/entity_type/entity_id/payload/user_id/created_at, and
-- every one of those keeps its exact old meaning (see services/
-- learning_event.rs's own header for the reader-compatibility note).
alter table learning_events rename to learning_events_legacy;

create table learning_events (
  id uuid not null default gen_random_uuid(),
  user_id uuid not null references users(id) on delete cascade,
  event_type text not null,
  entity_type text not null,
  entity_id uuid not null,
  payload jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  -- Which part of the app this happened in — ADR-0013 §1.5's table.
  source text not null default 'practice'
    check (source in ('self_learning', 'live_class', 'tryout', 'assignment', 'practice', 'speaking_room', 'live_ai_chat', 'canvas', 'review')),
  session_id uuid,
  org_id uuid references organizations(id) on delete set null,
  module_item_id uuid references module_items(id) on delete set null,
  -- A question's `uid` (P39-001) or a Modul Belajar section's id — TEXT,
  -- not uuid: a section id is a short hex fragment, not a real UUID
  -- (see lesson_plan::new_section_id), so this has to fit both shapes.
  content_uid text,
  content_version int,
  -- When the event actually happened on the client, separate from
  -- `created_at` (when the server wrote the row) — a batched client
  -- event (P39-005) can arrive seconds to minutes after occurring.
  occurred_at timestamptz not null default now(),
  -- The client's own id for this event, for de-duplicating a retried
  -- POST /events batch. Uniqueness is enforced via the SEPARATE
  -- learning_event_idempotency table below, NOT a constraint on this
  -- column — a partitioned table's unique constraints (PK included)
  -- must include the partition key, which would let a retry that lands
  -- one second later in a different `created_at` slip through as if it
  -- were a different event.
  client_event_id text,
  schema_version int not null default 1,
  primary key (id, created_at)
) partition by range (created_at);

create index learning_events_user_created_idx on learning_events (user_id, created_at);
create index learning_events_item_content_idx on learning_events (module_item_id, content_uid);
create index learning_events_type_created_idx on learning_events (event_type, created_at);

-- The ACTUAL uniqueness guarantee for client_event_id — small and NOT
-- partitioned, so a plain composite primary key can enforce it globally
-- regardless of which month the retried event's `created_at` lands in.
create table learning_event_idempotency (
  user_id uuid not null references users(id) on delete cascade,
  client_event_id text not null,
  event_id uuid not null,
  created_at timestamptz not null default now(),
  primary key (user_id, client_event_id)
);

-- Partitions covering the existing data's range plus the rest of this
-- year. `services/learning_event.rs::ensure_current_partitions` runs at
-- boot as a standing safety net for whenever the app is still live past
-- this — a proper monthly cron job belongs in Phase 40 once its job
-- queue exists; until then, "create this/next month's partition if
-- missing, every time the server starts" is enough not to fall over.
do $$
declare
  month date;
begin
  for month in select generate_series('2026-01-01'::date, '2026-12-01'::date, interval '1 month')::date loop
    execute format(
      'create table %I partition of learning_events for values from (%L) to (%L)',
      'learning_events_' || to_char(month, 'YYYY_MM'), month, month + interval '1 month'
    );
  end loop;
end $$;

-- Catches anything outside that pre-created range (clock skew, or the
-- boot-time safety net above not having run yet) — without this, an
-- insert into an unmapped date fails outright instead of degrading.
create table learning_events_default partition of learning_events default;

insert into learning_events (id, user_id, event_type, entity_type, entity_id, payload, created_at, occurred_at, source, schema_version)
select id, user_id, event_type, entity_type, entity_id, payload, created_at, created_at, 'practice', 1
from learning_events_legacy;

drop table learning_events_legacy;
