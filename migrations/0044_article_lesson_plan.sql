-- An `article` item becomes a "Modul Belajar": an ordered set of
-- sections, each with a title, a time estimate, a learning goal and an
-- ALM body — the shape parelabs stores as module_item.config.lesson_plan.
--
-- The plan is the source of truth for the editor and the learner's
-- reader. content_blocks stay, re-projected from the plan on every save,
-- so QA and every other reader of blocks keep working unchanged.
--
-- Null means a plain (pre-existing) article, edited as one ALM document.
alter table module_items add column lesson_plan jsonb;

-- A learner reads a module in the language THEY pick. Translating a
-- whole plan is several AI calls, so the result is kept per language
-- and reused until the source plan changes (source_hash).
create table module_item_translations (
  item_id uuid not null references module_items(id) on delete cascade,
  language text not null,
  source_hash text not null,
  lesson_plan jsonb not null,
  created_at timestamptz not null default now(),
  primary key (item_id, language)
);
