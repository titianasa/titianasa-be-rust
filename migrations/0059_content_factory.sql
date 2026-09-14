-- Pabrik Konten (ADR-0015): generating curriculum content from Admin
-- Pusat instead of from scripts on a developer's machine. A RUN is what
-- an admin asked for ("susun bab untuk Biologi Tahap 2"); it fans out
-- into TASKS, one unit of work each (a domain of topics for bab plans,
-- one bab for its content). Each task is carried by one `jobs` row, so
-- retries, reaping and concurrency are the queue's, not re-invented.
--
-- Every task goes generate → validate (rules) → QA (a stronger judge
-- model) → apply, repairing only what failed, and lands in
-- `needs_review` when repairs run out. Nothing here is applied without
-- either passing QA or a human approving it.

-- What a stage of a subject is written TO: the jenjang the language and
-- Bloom mix are calibrated for, and the standards the model is held to.
-- One row per Tahap folder of the canonical library; a stage without a
-- row can't be generated for (the dashboard says "perlu standar").
create table if not exists curriculum_standards (
  id uuid primary key default gen_random_uuid(),
  tahap_folder_id uuid not null unique references modules(id) on delete cascade,
  subject_id uuid references subjects(id) on delete set null,
  jenjang text not null,
  standar text[] not null default '{}',
  bahasa text not null default 'id',
  jenis_soal text not null default 'hitungan' check (jenis_soal in ('hitungan', 'konsep', 'bahasa')),
  catatan text,
  updated_by uuid references users(id) on delete set null,
  updated_at timestamptz not null default now()
);

-- Gold examples the generator is shown (few-shot) and a benchmark
-- compares against. Seeded from the bab plans Claude wrote; a human-
-- approved, high-scoring result can be promoted into one.
create table if not exists generation_exemplars (
  id uuid primary key default gen_random_uuid(),
  kind text not null,
  subject_id uuid references subjects(id) on delete set null,
  stage text,
  domain_folder_id uuid references modules(id) on delete set null,
  title text not null,
  input jsonb not null,
  output jsonb not null,
  source text not null check (source in ('claude', 'approved')),
  enabled boolean not null default true,
  created_at timestamptz not null default now(),
  unique (kind, domain_folder_id, source)
);

create table if not exists generation_runs (
  id uuid primary key default gen_random_uuid(),
  kind text not null,
  title text not null,
  scope jsonb not null,
  options jsonb not null default '{}'::jsonb,
  status text not null default 'queued' check (status in ('queued', 'running', 'paused', 'done', 'cancelled')),
  -- A benchmark run never applies anything: it generates for targets
  -- that already have a gold answer and scores the two blind.
  benchmark boolean not null default false,
  total_tasks int not null default 0,
  tokens_used bigint not null default 0,
  token_limit bigint,
  pause_reason text,
  created_by uuid references users(id) on delete set null,
  created_at timestamptz not null default now(),
  started_at timestamptz,
  finished_at timestamptz,
  updated_at timestamptz not null default now()
);
create index if not exists generation_runs_created_idx on generation_runs (created_at desc);

create table if not exists generation_tasks (
  id uuid primary key default gen_random_uuid(),
  run_id uuid not null references generation_runs(id) on delete cascade,
  kind text not null,
  target_type text not null check (target_type in ('domain', 'topic', 'bab')),
  target_id uuid not null,
  label text not null,
  status text not null default 'queued' check (status in (
    'queued', 'generating', 'validating', 'qa', 'applied', 'needs_review', 'rejected', 'failed', 'cancelled', 'benchmarked'
  )),
  -- Generate+repair rounds spent (not job retries — a transient
  -- provider error is the queue's business).
  attempt int not null default 0,
  prompt_version text,
  context jsonb,
  draft jsonb,
  validation jsonb,
  qa jsonb,
  -- One entry per round: what was wrong, what was asked to be fixed.
  history jsonb not null default '[]'::jsonb,
  tokens_generate bigint not null default 0,
  tokens_qa bigint not null default 0,
  error text,
  job_id uuid,
  reviewed_by uuid references users(id) on delete set null,
  reviewed_at timestamptz,
  review_note text,
  applied_at timestamptz,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);
create index if not exists generation_tasks_run_idx on generation_tasks (run_id, status);
create index if not exists generation_tasks_status_idx on generation_tasks (status, updated_at desc);
create index if not exists generation_tasks_target_idx on generation_tasks (target_id);
