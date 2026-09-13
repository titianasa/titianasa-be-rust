-- P40-002 (ADR-0013 §3.1, ADR-0014 §3) — one generic job queue, used
-- first for Admin Pusat's own daily-aggregate rollup (P40-004), later
-- for the semi-autonomous content agents (Fase 42). No new
-- infrastructure (no Redis/Kafka queue) — Postgres `FOR UPDATE SKIP
-- LOCKED` is enough at this scale, and it means a job and the rows it
-- reads/writes can share one transaction.
--
-- `priority`: LOWER claims first (matches nice(1) — 0 is most urgent),
-- default 100 so an explicit high-priority enqueue doesn't need every
-- other caller to also specify one.
--
-- State machine: pending -> running (claimed) -> done, OR running ->
-- pending again (retry, `run_after` pushed out) until `attempt` reaches
-- `max_attempts`, then -> failed. `reap` recovers a job stuck in
-- `running` (its worker crashed) back to `pending` without counting
-- that as a failed attempt — the job never actually ran to completion
-- or failure, so it shouldn't spend one of its retries.
create table jobs (
  id uuid primary key default gen_random_uuid(),
  job_type text not null,
  payload jsonb not null default '{}'::jsonb,
  priority integer not null default 100,
  status text not null default 'pending' check (status in ('pending', 'running', 'done', 'failed')),
  attempt integer not null default 0,
  max_attempts integer not null default 5,
  run_after timestamptz not null default now(),
  locked_by text,
  locked_at timestamptz,
  last_error text,
  -- Filled only by a job that actually spends AI tokens (Fase 42's
  -- content agents) — most job types (this ticket's rollups) leave it
  -- null. Read by Operasional AI (P40-005) alongside `ai_tasks`.
  cost_tokens integer,
  created_at timestamptz not null default now(),
  finished_at timestamptz
);

-- The one query the claimer and the health-check both run: "next
-- runnable job of a type I handle, oldest/most-urgent first."
create index jobs_status_run_after_priority_idx on jobs (status, run_after, priority);
