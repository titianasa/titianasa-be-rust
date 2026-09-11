-- migrations/0032_subject_preset_folders.sql
-- Phase 34 follow-up — a second preset category alongside the
-- curriculum/exam-track folders from 0030/0031: per-subject folders
-- (Matematika, IPA, IPS, Ekonomi, Sejarah, Geografi, Fisika, Kimia,
-- Biologi, Filsafat) for a learner who wants one subject end-to-end,
-- not split by grade level. Same pattern as before — plain organizing
-- folders (is_folder=true, no subject_id, no fabricated content),
-- fixed literal ids so the frontend picker can link to them directly.
insert into "modules" ("id", "parent_id", "is_folder", "title", "description", "order_index") values
	('5032dc43-03ff-4bb4-806d-a31148400509', null, true, 'Semua Mata Pelajaran', 'Belajar satu mata pelajaran dari awal sampai akhir, tanpa dibagi jenjang.', 106);
--> statement-breakpoint

insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('112b37d1-ddd8-42c8-be8d-08c511acd2e7', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Matematika', 0),
	('46495a3f-0a9c-4847-994a-ebe8f2d78d95', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'IPA', 1),
	('de98928c-6f65-40c5-8c8a-342d792ed0f6', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'IPS', 2),
	('449191f4-0f35-46a0-8bfb-8a02341bdcf6', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Ekonomi', 3),
	('31011bd2-3d1b-419e-adab-c2195c6a6459', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Sejarah', 4),
	('ecf20672-6938-4136-9c1d-4cdcaee777f1', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Geografi', 5),
	('ad3aa5d6-7d09-45e6-a9c4-aff4c3a76cea', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Fisika', 6),
	('6f858291-f8db-4dea-8672-f99df4d3d2fb', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Kimia', 7),
	('4d66f43a-ff83-4b6d-bfe4-64ed1a58d9bd', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Biologi', 8),
	('dd37f594-b3d3-431f-ba2f-1735c3490130', '5032dc43-03ff-4bb4-806d-a31148400509', true, 'Filsafat', 9);
