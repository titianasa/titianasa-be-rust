-- P40-001 (ADR-0014 §1) — every Admin Pusat write action (approve/reject
-- an AI proposal, change AI model settings, toggle a catalog entry,
-- roll back a content version, ...) leaves a permanent row here: who,
-- when, what, and — since this is the ONE place in the product where an
-- action needs a human-readable justification attached — why.
--
-- `before`/`after` are whole-row jsonb snapshots of whatever
-- `target_type`/`target_id` point at, not a diff — a diff is derivable
-- from the two, but reconstructing "what it looked like before" from a
-- diff alone is not. Both nullable: a pure creation has no `before`, a
-- pure deletion no `after`.
create table admin_audit_log (
  id uuid primary key default gen_random_uuid(),
  -- Nullable + `set null` (not `cascade`, unlike most user_id FKs in
  -- this codebase) — deleting the actor's account must never erase
  -- what they did; the row survives with actor_id=null.
  actor_id uuid references users(id) on delete set null,
  action text not null,
  target_type text not null,
  target_id uuid,
  before jsonb,
  after jsonb,
  reason text,
  created_at timestamptz not null default now()
);

create index admin_audit_log_created_at_idx on admin_audit_log (created_at desc, id desc);
create index admin_audit_log_target_idx on admin_audit_log (target_type, target_id);
