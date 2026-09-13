-- P39-002 (ADR-0013 L1) — a permanent version history for the NEW
-- content model (`module_items.lesson_plan`/`quiz_config`, Phase 37/38),
-- extending ADR-0008's versioning idea (which only ever covered the
-- older `questions`/`lessons` bank) to what authoring actually writes
-- to today. A row is frozen only when an item is PUBLISHED
-- (module_item::publish) — the draft being edited right now has no
-- version row yet, same "editing a draft is free, publishing freezes
-- it" split ADR-0008 already established.
--
-- This is the foundation ADR-0013's content optimizer needs to ever say
-- "v1.1 scored better than v1.0": without a frozen snapshot per publish,
-- there is nothing stable to attribute a learner's answer to once the
-- author edits the item again.
create table module_item_versions (
  id uuid primary key default gen_random_uuid(),
  item_id uuid not null references module_items(id) on delete cascade,
  version int not null,
  lesson_plan jsonb,
  quiz_config jsonb,
  -- A content fingerprint (sha256 hex, computed in Rust — see
  -- module_item_version.rs), cheap to compare so a caller can tell two
  -- versions apart without diffing potentially-large jsonb blobs.
  content_hash text not null,
  created_by uuid references users(id) on delete set null,
  -- 'ai_proposal' isn't produced by any code path yet — modeled ahead
  -- of Phase 42's content-agent proposals so this column never needs a
  -- second migration just to widen its CHECK constraint.
  created_via text not null default 'human' check (created_via in ('human', 'ai_generation', 'ai_proposal')),
  change_summary text,
  published_at timestamptz not null default now(),
  superseded_at timestamptz,
  unique (item_id, version)
);
create index module_item_versions_item_idx on module_item_versions (item_id, version desc);

-- Null = never published, or published before this migration and not
-- yet backfilled below (only possible for a legacy plain-ALM article —
-- see the backfill's own WHERE clause).
alter table module_items add column current_version int;

-- Backfill: every item that's ALREADY published today, and actually has
-- content in the new shape (lesson_plan or quiz_config), gets version 1
-- — it was authored and published before this table existed, but the
-- content sitting in module_items right now IS what every learner has
-- been attempting/reading, so that's what version 1 has to be.
--
-- Deliberately excludes plain legacy articles (both columns null —
-- their content lives in content_blocks only, migration 0044's own
-- note: "Null means a plain (pre-existing) article, edited as one ALM
-- document") — this table versions the new content model, not the old
-- content_blocks-only one.
insert into module_item_versions (item_id, version, lesson_plan, quiz_config, content_hash, created_via, published_at)
select id, 1, lesson_plan, quiz_config, md5(coalesce(lesson_plan::text, '') || coalesce(quiz_config::text, '')), 'human', updated_at
from module_items
where status = 'published' and (lesson_plan is not null or quiz_config is not null);

update module_items set current_version = 1
where status = 'published' and (lesson_plan is not null or quiz_config is not null);

-- P39-002 — "attempt yang dibuat di antara keduanya mencatat versi yang
-- berlaku saat itu": which frozen version an attempt was actually
-- scored against, read from module_items.current_version in the SAME
-- query as the quiz_config used to grade it (quiz_attempt.rs). Null for
-- attempts on an item that had no frozen version yet (or predates this
-- column) — an honest "unknown", never a guessed 1.
alter table attempts add column content_version int;
