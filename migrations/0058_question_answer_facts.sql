-- One row per graded answer, carrying everything a heatmap slices by:
-- where the question sits in the curriculum (subject, folder path —
-- Tahap, domain… — topik, bab, the Modul Belajar section it was written
-- from) and what kind of question it is (subtype, difficulty, Bloom).
--
-- Snapshotted at answer time rather than joined later: a bab renamed or
-- a question re-labelled next month must not rewrite what a learner's
-- history says they practised. `learning_events` keeps the raw event;
-- this is the analytic shape of the same fact (ADR-0013 L1 → L2).
create table if not exists question_answer_facts (
  id uuid not null default gen_random_uuid(),
  user_id uuid not null references users(id) on delete cascade,
  answered_at timestamptz not null default now(),
  source text not null check (source in ('practice', 'tryout', 'checkpoint')),
  attempt_id uuid,
  -- The item the learner sat (a pooled Latihan, a Modul Belajar) and the
  -- item the question actually lives in (the bank) — often different.
  module_item_id uuid not null,
  content_item_id uuid not null,
  question_uid uuid not null,
  content_version int,
  subject_id uuid,
  -- Folder ancestors from the root down to AND INCLUDING the topik
  -- module (content_context.rs). Depth varies by subject, so drill-down
  -- walks this array instead of assuming "level 2 is Tahap".
  folder_path uuid[] not null default '{}',
  module_id uuid not null,
  bab_id uuid,
  source_section_id text,
  subtype text not null,
  difficulty text,
  bloom text,
  -- NULL: not auto-graded (manual / AI rubric pending).
  correct boolean,
  points_earned real not null default 0,
  points_max real not null default 0,
  primary key (id, answered_at)
) partition by range (answered_at);

create index if not exists question_answer_facts_user_idx on question_answer_facts (user_id, answered_at);
create index if not exists question_answer_facts_module_idx on question_answer_facts (module_id, answered_at);
create index if not exists question_answer_facts_question_idx on question_answer_facts (question_uid);

do $$
declare
  month date;
begin
  for month in select generate_series('2026-01-01'::date, '2027-12-01'::date, interval '1 month')::date loop
    execute format(
      'create table if not exists %I partition of question_answer_facts for values from (%L) to (%L)',
      'question_answer_facts_' || to_char(month, 'YYYY_MM'), month, month + interval '1 month'
    );
  end loop;
end $$;
create table if not exists question_answer_facts_default partition of question_answer_facts default;

-- Admin Pusat never scans the facts table when a page opens (ADR-0014):
-- per WIB day × question, recomputed by the metrics_rollup job. Every
-- heatmap level (subject → folders → topik → bab) and "soal tersulit"
-- is a SUM over this, since a question's placement is constant.
create table if not exists metrics_daily_question (
  day date not null,
  question_uid uuid not null,
  content_item_id uuid not null,
  subject_id uuid,
  folder_path uuid[] not null default '{}',
  module_id uuid not null,
  bab_id uuid,
  subtype text not null,
  difficulty text,
  bloom text,
  answered int not null default 0,
  graded int not null default 0,
  correct int not null default 0,
  learners int not null default 0,
  timezone text not null default 'Asia/Jakarta',
  primary key (day, question_uid)
);
create index if not exists metrics_daily_question_module_idx on metrics_daily_question (module_id, day);
