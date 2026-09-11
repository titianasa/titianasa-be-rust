-- migrations/0033_preset_folder_subtrees.sql
-- Phase 34 follow-up 2 — the 6 preset folders that only had Kurikulum
-- Indonesia and Semua Mata Pelajaran as siblings with real sub-folder
-- breakdowns (0030/0032) now get their own, so every preset on the
-- dashboard picker shows a real structure once clicked in, not just a
-- lone empty folder. Same pattern throughout: plain organizing folders
-- (is_folder=true, no subject_id, no fabricated lesson content), fixed
-- literal ids, no picker-side code changes needed (ModuleBrowser
-- already lists a folder's children the moment you're inside it).

-- Belajar Bahasa Inggris -> CEFR-style level bands.
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('0414f34f-fad8-4553-9be0-b67c4bdbd79a', '319a76f8-4900-4d3e-87ce-104016f2ddef', true, 'Pre-Basic', 0),
	('b0c4f357-82b0-417d-b84d-daefa2d05878', '319a76f8-4900-4d3e-87ce-104016f2ddef', true, 'A1 - Basic', 1),
	('ab501e71-34a0-43c1-a11b-70cab39d45e2', '319a76f8-4900-4d3e-87ce-104016f2ddef', true, 'A2 - Elementary', 2),
	('bb27c4fb-ce2f-4ef1-a347-249c536542f2', '319a76f8-4900-4d3e-87ce-104016f2ddef', true, 'B1 - Intermediate', 3),
	('60f0a6d2-e1dd-46b0-bb65-9f187e71ee61', '319a76f8-4900-4d3e-87ce-104016f2ddef', true, 'B2 - Upper Intermediate', 4),
	('9398f5b8-b250-4cb2-b3f8-127cb06232d7', '319a76f8-4900-4d3e-87ce-104016f2ddef', true, 'C1 - Advanced', 5),
	('fdc4cefd-22cb-4549-bc2f-a2ac7b5e49e7', '319a76f8-4900-4d3e-87ce-104016f2ddef', true, 'C2 - Proficiency', 6);
--> statement-breakpoint

-- Kurikulum Cambridge -> its own real stage groupings.
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('5a5af013-c468-4823-889f-b72bfa511d1a', '192268cd-8a52-4ffc-9009-0e423d1052a1', true, 'Cambridge Primary', 0),
	('5b06d044-8ea8-4411-a60d-bd342951ea3f', '192268cd-8a52-4ffc-9009-0e423d1052a1', true, 'Cambridge Lower Secondary', 1),
	('17ef9979-88c7-4c87-ab31-8b57f8f02de2', '192268cd-8a52-4ffc-9009-0e423d1052a1', true, 'Cambridge Upper Secondary (IGCSE)', 2),
	('850025dd-4f36-45a6-93c0-58db2e0e277b', '192268cd-8a52-4ffc-9009-0e423d1052a1', true, 'Cambridge Advanced (A Level)', 3);
--> statement-breakpoint

-- Fokus Ujian TKA -> per jenjang.
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('de5a11f2-b91b-463d-95e4-a0e1428c460e', '22f81985-aeaa-4b5d-b82c-c87440ac2948', true, 'TKA SD', 0),
	('c612e5ea-d903-4e74-b4db-3c7601b61cdb', '22f81985-aeaa-4b5d-b82c-c87440ac2948', true, 'TKA SMP', 1),
	('a65cf72f-4a7b-4ed3-bca7-3ff8a60522cc', '22f81985-aeaa-4b5d-b82c-c87440ac2948', true, 'TKA SMA', 2);
--> statement-breakpoint

-- Ujian Masuk Perguruan Tinggi -> jalur seleksi.
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('6de9068f-4706-44de-9a43-f9ba9afd4dfc', '89a6650a-1c4c-4e95-93f8-0354a1585a8a', true, 'UTBK-SNBT Saintek', 0),
	('4eb96d87-8af6-44e8-8e85-1754982cd77c', '89a6650a-1c4c-4e95-93f8-0354a1585a8a', true, 'UTBK-SNBT Soshum', 1),
	('b80c5534-fda8-49f1-bf9f-aea0fe777b68', '89a6650a-1c4c-4e95-93f8-0354a1585a8a', true, 'Ujian Mandiri PTN', 2);
--> statement-breakpoint

-- Ujian CPNS -> tahapan seleksi nyata (SKD punya 3 sub-tes, lalu SKB).
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('10dda6b7-41b7-4f17-a218-e5d756dd8118', '51912d15-fb91-4f91-94fa-161fd4413454', true, 'SKD - Tes Wawasan Kebangsaan (TWK)', 0),
	('4a0bdb26-18b0-46c2-90d1-36f8352c9477', '51912d15-fb91-4f91-94fa-161fd4413454', true, 'SKD - Tes Intelegensia Umum (TIU)', 1),
	('7f34a2ce-6be0-4019-9105-9918c1a36b07', '51912d15-fb91-4f91-94fa-161fd4413454', true, 'SKD - Tes Karakteristik Pribadi (TKP)', 2),
	('b6ff49e0-ea86-4e2c-a9fd-77b24fd02d41', '51912d15-fb91-4f91-94fa-161fd4413454', true, 'SKB', 3);
--> statement-breakpoint

-- Ujian BUMN -> tahapan seleksi Rekrutmen Bersama BUMN.
insert into "modules" ("id", "parent_id", "is_folder", "title", "order_index") values
	('a27164f8-4571-4a93-a471-56535bb92531', 'f2a5bf42-1806-45cf-a717-09098351590f', true, 'Tes Kemampuan Dasar (TKD)', 0),
	('61a250b2-aee3-42f0-995d-71949e3b60f9', 'f2a5bf42-1806-45cf-a717-09098351590f', true, 'Tes Bahasa Inggris', 1),
	('b63fa92f-d780-42cc-b479-605a3173eb54', 'f2a5bf42-1806-45cf-a717-09098351590f', true, 'Tes AKHLAK', 2);
