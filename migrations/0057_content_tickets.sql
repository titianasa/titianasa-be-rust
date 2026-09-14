-- Learners (and anyone previewing) flag a question, a checkpoint
-- question or a Modul Belajar section that looks wrong: a typo, a wrong
-- key, choices with no right answer, a broken table. Each flag is a
-- REPORT; reports about the same piece of content collect into one
-- TICKET, which is what Admin Pusat triages. Ten learners hitting the
-- same wrong key is one problem with ten votes, not ten tickets.
create table if not exists content_tickets (
  id uuid primary key default gen_random_uuid(),
  -- Where the content lives, i.e. where it gets FIXED: for a pooled
  -- Latihan that is the bank item, not the item the learner was in.
  module_item_id uuid not null references module_items(id) on delete cascade,
  target_type text not null check (target_type in ('question', 'checkpoint_question', 'section')),
  -- A question's uid (P39-001) or a lesson_plan section id.
  content_uid text not null,
  status text not null default 'open' check (status in ('open', 'in_review', 'resolved', 'rejected')),
  report_count int not null default 0,
  categories text[] not null default '{}',
  first_reported_at timestamptz not null default now(),
  last_reported_at timestamptz not null default now(),
  resolution_note text,
  handled_by uuid references users(id) on delete set null,
  handled_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

-- At most one ACTIVE ticket per piece of content; once resolved or
-- rejected, a new report opens a fresh one (the fix may not have held).
create unique index if not exists content_tickets_one_active
  on content_tickets (module_item_id, target_type, content_uid) where status in ('open', 'in_review');
create index if not exists content_tickets_status_idx on content_tickets (status, last_reported_at desc);

create table if not exists content_reports (
  id uuid primary key default gen_random_uuid(),
  ticket_id uuid not null references content_tickets(id) on delete cascade,
  reporter_id uuid references users(id) on delete set null,
  reporter_role text,
  -- The item the learner was in when they flagged it (a pooled Latihan,
  -- a preview) — may differ from the ticket's item.
  reported_from_item_id uuid references module_items(id) on delete set null,
  category text not null check (category in ('typo', 'wrong_answer_key', 'bad_choices', 'unclear', 'incorrect_content', 'display_broken', 'other')),
  message text check (char_length(message) <= 1000),
  -- What the learner actually SAW: shown number, re-lettered choices,
  -- their answer. A wrong-key report is only checkable against the
  -- letters on their screen, not the stored ones.
  context jsonb not null default '{}'::jsonb,
  content_version int,
  created_at timestamptz not null default now(),
  -- One report per person per ticket: flagging again updates it.
  unique (ticket_id, reporter_id)
);
create index if not exists content_reports_reporter_idx on content_reports (reporter_id, created_at);
