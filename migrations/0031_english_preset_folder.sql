-- migrations/0031_english_preset_folder.sql
-- Phase 34 follow-up — "Belajar Bahasa Inggris" needs its own root
-- folder too, not the bare root listing: linking that preset to
-- `/belajar` with no parent (0030's original design) meant it showed
-- every OTHER root module too, including the 6 preset folders 0030
-- just created (Kurikulum Indonesia, Cambridge, etc. would show up as
-- siblings on a page meant to be English-only). Symmetric with the
-- other 6 presets now.
insert into "modules" ("id", "parent_id", "is_folder", "title", "description", "order_index") values
	('319a76f8-4900-4d3e-87ce-104016f2ddef', null, true, 'Belajar Bahasa Inggris', 'Semua materi Bahasa Inggris.', 99);
