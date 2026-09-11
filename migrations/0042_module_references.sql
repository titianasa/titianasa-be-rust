-- Learning paths stop owning copies of the curriculum.
--
-- Until now every learning path (Kurikulum Indonesia, Cambridge, CPNS,
-- BUMN, TKA, UTBK, Belajar Bahasa Inggris) carried its OWN duplicate
-- topic rows — ~744 of them — parallel to the master library in
-- "Semua Mata Pelajaran". Editing a topic in the library never reached
-- the copy inside a path, and adding a subject to a path meant building
-- the material a second time.
--
-- A reference row is a module that owns no content of its own: it sits
-- in the path's tree purely to say "here, at this position, show that
-- library module". Content (module_items, labels, status) is always
-- read from the source, so the library stays the single source of
-- truth and a path becomes a curated VIEW over it.
alter table modules add column source_module_id uuid references modules(id) on delete restrict;

-- Referencing a reference would make resolution recursive for no gain —
-- a path always points straight at the library module it wants. This is
-- enforced at write time in module.rs (a source that itself has a
-- source is rejected); the index below keeps that check cheap.
create index modules_source_module_id_idx on modules (source_module_id) where source_module_id is not null;

-- ON DELETE RESTRICT above is the important half: a library module that
-- some path still points at cannot be deleted out from under it. The
-- curator has to detach the reference first, which is exactly the
-- safeguard the old duplicate-copy arrangement never had.
