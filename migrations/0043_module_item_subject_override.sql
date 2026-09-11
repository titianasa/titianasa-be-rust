-- An item's subject defaults to its module's (the badge/hint/AI-prompt
-- logic all read module_items.module_id -> modules.subject_id today).
-- That's right almost always, but not for a module that's a mixed
-- container by design (a "latihan campuran" holding one Matematika
-- article next to one Fisika article) — this override lets ONE item
-- diverge from its module without splitting the module apart.
--
-- Null means "inherit from the parent module" (the status quo,
-- unchanged for every existing item); non-null wins over the module's.
alter table module_items
  add column subject_id uuid references subjects(id) on delete set null;
