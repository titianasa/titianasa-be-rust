-- migrations/0030_learning_preset_folders.sql
-- Phase 34 — organizational folders backing the dashboard's "Pilih
-- fokus belajar" preset picker (titian-web's
-- components/dashboard/learning-preset-picker.tsx). Pure folders
-- (is_folder=true, no subject_id, no content) — NOT fabricated
-- curriculum content. A teacher populates each with real modules later
-- via the existing Studio "Buat Modul" flow; until then the existing
-- /belajar browser's own "Belum ada materi di sini" empty state covers
-- it honestly. IDs are fixed literals (not gen_random_uuid()) because
-- the frontend picker links to them by a hardcoded id, same pattern
-- lib/curriculum.ts's DEFAULT_SUBJECT_ID already uses for the one real
-- subject. order_index starts at 100 so these seeded roots sort after
-- any organically created root modules/folders, not ahead of them.
insert into "modules" ("id", "parent_id", "is_folder", "title", "description", "order_index") values
	('7bd402a2-24cf-45a0-9698-fd972ae941ad', null, true, 'Kurikulum Indonesia', 'Materi mengikuti kurikulum nasional, per jenjang kelas.', 100),
	('192268cd-8a52-4ffc-9009-0e423d1052a1', null, true, 'Kurikulum Cambridge', 'Materi mengikuti kurikulum Cambridge.', 101),
	('22f81985-aeaa-4b5d-b82c-c87440ac2948', null, true, 'Fokus Ujian TKA', 'Latihan terfokus untuk Tes Kemampuan Akademik.', 102),
	('89a6650a-1c4c-4e95-93f8-0354a1585a8a', null, true, 'Persiapan Ujian Masuk Perguruan Tinggi', 'Latihan terfokus untuk ujian masuk perguruan tinggi.', 103),
	('51912d15-fb91-4f91-94fa-161fd4413454', null, true, 'Fokus Ujian CPNS', 'Latihan terfokus untuk seleksi CPNS.', 104),
	('f2a5bf42-1806-45cf-a717-09098351590f', null, true, 'Fokus Ujian BUMN', 'Latihan terfokus untuk seleksi rekrutmen BUMN.', 105);
--> statement-breakpoint

-- Kurikulum Indonesia's 12 grade-level sub-folders, SD 1 through SMA 3.
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('0ee5ad4b-b743-40e1-8bb5-cb6d12ee24ac', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SD Kelas 1', 0),
	('df3d397c-2e35-4b77-b520-d000f9d9d9c3', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SD Kelas 2', 1),
	('f6ddc058-4cc8-4408-8edc-e9b7fe1d8336', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SD Kelas 3', 2),
	('fac838d0-f781-4e44-bf9e-a2ef083f1518', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SD Kelas 4', 3),
	('6f6c2d11-5d1b-4fb8-bf13-1c1edc1846d4', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SD Kelas 5', 4),
	('0a8d4156-2793-44fb-86e0-99dce34e95e8', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SD Kelas 6', 5),
	('630987f8-a325-4e49-acd5-22253ce0ebd8', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SMP Kelas 1', 6),
	('18773afc-7054-4377-b245-b15cd56e1bb6', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SMP Kelas 2', 7),
	('1a54d861-e66d-4c53-813d-ba3440062497', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SMP Kelas 3', 8),
	('877b7a11-3d64-4bbd-a3cc-138bbecdf1ee', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SMA Kelas 1', 9),
	('067792b8-fe50-4126-83a2-c4a5b16d16cb', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SMA Kelas 2', 10),
	('28d33ff8-6699-4ee3-8362-a599f84ce94c', '7bd402a2-24cf-45a0-9698-fd972ae941ad', true, 'SMA Kelas 3', 11);
