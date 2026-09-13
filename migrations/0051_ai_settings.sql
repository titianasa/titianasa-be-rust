-- P40-003 (ADR-0013 §3.5, ADR-0014 §2 "Pengaturan AI") — every AI role
-- becomes admin-configurable, with a working platform even before any
-- admin ever touches this: `ai_role_settings` starts empty (no seed
-- rows) and `services/ai_settings.rs::resolve` falls through to the
-- SAME `Config.ai_*_model` env-backed field each call site already
-- reads today when a role has no row here. `ai_role_settings.role` is
-- plain text, not a DB enum or FK — the set of valid roles is the
-- code-defined `ai_settings::AI_ROLES` registry (same "no DB enum"
-- choice already made for `jobs.job_type` and `learning_events.event_type`).

create table ai_model_catalog (
  id uuid primary key default gen_random_uuid(),
  provider text not null,
  model_id text not null,
  label text not null,
  -- Subset of {"text","vision","json","stt","tts"} — checked against a
  -- role's required capabilities before that model can be assigned to
  -- it (services/ai_settings.rs, not a CHECK constraint: the valid set
  -- and the "does this role need X" rule both live in code).
  capabilities text[] not null default '{}',
  max_output_tokens integer,
  price_input_per_mtok_idr integer,
  price_output_per_mtok_idr integer,
  enabled boolean not null default true,
  created_at timestamptz not null default now(),
  unique (provider, model_id)
);

-- Seed: exactly what's already live today (ADR-0013 §3.5) — this
-- migration changes nothing about which models actually get called
-- until an admin edits `ai_role_settings`.
insert into ai_model_catalog (provider, model_id, label, capabilities, max_output_tokens) values
  ('vertex', 'gemini-3.8-flash', 'Gemini 3.8 Flash (Vertex AI)', array['text', 'vision', 'json'], 65536),
  ('openrouter', 'openai/whisper-1', 'Whisper v1 (OpenRouter)', array['stt'], null),
  ('openrouter', 'hexgrad/kokoro-82m', 'Kokoro 82M (OpenRouter)', array['tts'], null);

create table ai_role_settings (
  role text primary key,
  -- Both null = "no override, use Config's env-backed fallback for
  -- this role" (see `ai_settings::resolve`'s doc comment for the exact
  -- order). Not a foreign key into ai_model_catalog: validated at save
  -- time in application code instead (services/ai_settings.rs), since
  -- the rule also needs the role's required capabilities, which a plain
  -- FK can't express.
  model_id text,
  fallback_model_id text,
  temperature double precision,
  max_tokens integer,
  daily_token_budget integer,
  enabled boolean not null default true,
  updated_by uuid references users(id) on delete set null,
  updated_at timestamptz not null default now()
);
