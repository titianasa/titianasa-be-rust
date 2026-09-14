-- Per learner, per Modul Belajar section: where they stand on that
-- section's comprehension checkpoint (services/section_checkpoint.rs).
-- Client telemetry (`section_read`) can't be the source of truth for a
-- gate — it is consent-gated and client-reported — so the gate lives here.
--
--   open    a draw is waiting to be answered (`draw` holds the paper)
--   reread  a wrong answer locked the section; `locked_at` starts the
--           minimum re-read time before a new draw is allowed
--   passed  every drawn question answered correctly
--
-- Keyed by section_id (stable across edits/reorders/translation). A
-- whole-plan regeneration mints new ids, which orphans these rows — the
-- right outcome, since the content they certified is gone.
create table section_checkpoint_progress (
  user_id uuid not null references users(id) on delete cascade,
  item_id uuid not null references module_items(id) on delete cascade,
  section_id text not null,
  status text not null default 'open' check (status in ('open', 'reread', 'passed')),
  draw jsonb,
  attempts integer not null default 0,
  locked_at timestamptz,
  passed_at timestamptz,
  updated_at timestamptz not null default now(),
  primary key (user_id, item_id, section_id)
);
