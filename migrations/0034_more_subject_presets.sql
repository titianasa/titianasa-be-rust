-- migrations/0034_more_subject_presets.sql
-- Phase 34 follow-up 3 — 4 more subjects under "Semua Mata Pelajaran"
-- (0032's root, 5032dc43-03ff-4bb4-806d-a31148400509): Sosiologi,
-- Bahasa Indonesia, Pendidikan Kewarganegaraan, Akuntansi. Same
-- pattern as the other 10 — plain organizing folder, fixed literal id,
-- order_index continues after Filsafat (9).
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('eb0dbce4-1d6d-4d49-a529-f85b148215e2', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Sosiologi', 10),
	('c6b3ad96-baba-4f5e-b651-590fb38cddbd', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Bahasa Indonesia', 11),
	('f4786c94-a4c4-4999-be67-e8977a591417', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Pendidikan Kewarganegaraan', 12),
	('ee467ca5-5bdf-467d-8767-afb0ee46bfaf', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Akuntansi', 13);
