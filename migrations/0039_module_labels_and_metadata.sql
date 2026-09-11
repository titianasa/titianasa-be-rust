-- Curriculum mapping layer.
--
-- Problem this solves: every learning path (Kurikulum Indonesia, TKA,
-- UTBK-SNBT, CPNS, BUMN, Cambridge) currently owns a SEPARATE COPY of
-- its topic modules. The same "Turunan Fungsi" exists once under SMA
-- Kelas 12 and again under UTBK — siloed, and editing one never
-- reaches the other.
--
-- `module_labels` makes the master library ("Semua Mata Pelajaran") the
-- single source of truth and lets a path be ASSEMBLED by query instead
-- of by duplication: label a topic `jenjang=SMA-12` + `ujian=utbk-pm`
-- and it can surface in both places from one row. It reads both ways —
-- "where does this topic appear?" and "give me every topic for UTBK-PM".
--
-- `source` mirrors the existing generated_by convention (human/ai) so an
-- autonomous curriculum agent's labels stay distinguishable from a
-- human curator's, and can be reviewed or rolled back separately.
create table module_labels (
    id uuid primary key default gen_random_uuid(),
    module_id uuid not null references modules(id) on delete cascade,
    kind text not null,
    value text not null,
    source text not null default 'human',
    created_at timestamp with time zone not null default now(),
    constraint module_labels_source_check check (source in ('human', 'ai')),
    constraint module_labels_unique unique (module_id, kind, value)
);

-- The reverse lookup ("every module tagged ujian=utbk-pm") is the whole
-- point of the table, so it gets its own index rather than relying on
-- the unique constraint's module_id-leading order.
create index module_labels_kind_value_idx on module_labels (kind, value);

-- Open-ended metadata for the curriculum agent: learning objectives,
-- difficulty, estimated duration, source references, generation
-- parameters. Deliberately jsonb and deliberately unconstrained — an
-- agent must be able to enrich a module with NEW fields without a
-- schema migration, which is what keeps this structure extensible
-- rather than frozen.
alter table modules add column metadata jsonb;
