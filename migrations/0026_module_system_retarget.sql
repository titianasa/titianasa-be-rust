-- migrations/0026_module_system_retarget.sql
-- Phase 31 (P31-002) — run AFTER 0025_module_system.sql. Retargets the
-- 4 tables outside the old curriculum hierarchy that FK into
-- lessons.id, then drops the old hierarchy itself.
--
-- CORRECTION to the original plan's premise: "zero production data"
-- was true of the checked-in migrations/seeds (confirmed by grep before
-- this migration was written), but NOT true of the live dev database —
-- this session's own smoke/verification testing had accumulated real
-- rows (1 curriculum, 12 lessons, 198 content_blocks, 11 attempts, 3
-- canvas_sessions) that would otherwise violate the new item_id FKs
-- below. Backed up first (data-only pg_dump of all 9 affected tables,
-- /home/john/backups/titian-pre-module-migration-data-20260906.sql)
-- before this migration clears them — this is still a clean replace
-- (the plan's actual intent), just with a safety net the original
-- "nothing to lose" premise skipped. `attempts` is scoped narrowly
-- (only its lesson-linked rows) since it also holds unrelated
-- assessment-linked rows that must survive untouched.
DELETE FROM "content_blocks";
--> statement-breakpoint
-- evaluations/feedback (AI writing/speaking evaluation rows, R12) FK
-- into attempts — must clear these before the lesson-linked attempts
-- rows they belong to, scoped so assessment-linked attempts/evaluations
-- (unaffected by this migration) survive untouched.
DELETE FROM "feedback" WHERE "evaluation_id" IN (
	SELECT e."id" FROM "evaluations" e JOIN "attempts" a ON a."id" = e."attempt_id" WHERE a."lesson_id" IS NOT NULL
);
--> statement-breakpoint
DELETE FROM "evaluations" WHERE "attempt_id" IN (
	SELECT "id" FROM "attempts" WHERE "lesson_id" IS NOT NULL
);
--> statement-breakpoint
-- canvas_events/canvas_snapshots (P26-001 children of canvas_sessions)
-- must go before canvas_sessions itself, which must go before attempts
-- (submitted_attempt_id FK).
DELETE FROM "canvas_events";
--> statement-breakpoint
DELETE FROM "canvas_snapshots";
--> statement-breakpoint
DELETE FROM "canvas_sessions";
--> statement-breakpoint
DELETE FROM "attempts" WHERE "lesson_id" IS NOT NULL;
--> statement-breakpoint
DELETE FROM "lesson_completion_overrides";
--> statement-breakpoint
DELETE FROM "lesson_concepts";
--> statement-breakpoint

-- content_blocks.lesson_id -> item_id (FK -> module_items)
ALTER TABLE "content_blocks" DROP CONSTRAINT "content_blocks_lesson_id_lessons_id_fk";
--> statement-breakpoint
ALTER TABLE "content_blocks" RENAME COLUMN "lesson_id" TO "item_id";
--> statement-breakpoint

-- attempts.lesson_id -> item_id (FK -> module_items)
ALTER TABLE "attempts" DROP CONSTRAINT "attempts_lesson_id_lessons_id_fk";
--> statement-breakpoint
ALTER TABLE "attempts" RENAME COLUMN "lesson_id" TO "item_id";
--> statement-breakpoint

-- canvas_sessions.lesson_id -> item_id (FK -> module_items). NOTE:
-- canvas_sessions is the existing, UNRELATED P26-001 tutor/student
-- writing-feedback feature — not the new M3 collaboration system. Same
-- word "canvas," different feature; only the FK target changes here.
ALTER TABLE "canvas_sessions" DROP CONSTRAINT "canvas_sessions_lesson_id_lessons_id_fk";
--> statement-breakpoint
ALTER TABLE "canvas_sessions" RENAME COLUMN "lesson_id" TO "item_id";
--> statement-breakpoint

-- lesson_completion_overrides -> item_completion_overrides (table +
-- column rename; ADR-0012 Module Completion Rule logic is unchanged).
ALTER TABLE "lesson_completion_overrides" DROP CONSTRAINT "lesson_completion_overrides_lesson_id_lessons_id_fk";
--> statement-breakpoint
ALTER TABLE "lesson_completion_overrides" RENAME COLUMN "lesson_id" TO "item_id";
--> statement-breakpoint
ALTER TABLE "lesson_completion_overrides" RENAME TO "item_completion_overrides";
--> statement-breakpoint

-- Now that nothing references lessons.id anymore, drop the old
-- hierarchy. CASCADE takes lesson_concepts (superseded by
-- module_item_concepts) and any FK from lessons.superseded_by along
-- with it.
DROP TABLE "lessons" CASCADE;
--> statement-breakpoint
DROP TABLE "units" CASCADE;
--> statement-breakpoint
DROP TABLE "levels" CASCADE;
--> statement-breakpoint
DROP TABLE "curricula" CASCADE;
--> statement-breakpoint

-- New FKs onto module_items for the 4 retargeted tables.
ALTER TABLE "content_blocks" ADD CONSTRAINT "content_blocks_item_id_module_items_id_fk" FOREIGN KEY ("item_id") REFERENCES "public"."module_items"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "attempts" ADD CONSTRAINT "attempts_item_id_module_items_id_fk" FOREIGN KEY ("item_id") REFERENCES "public"."module_items"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "canvas_sessions" ADD CONSTRAINT "canvas_sessions_item_id_module_items_id_fk" FOREIGN KEY ("item_id") REFERENCES "public"."module_items"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "item_completion_overrides" ADD CONSTRAINT "item_completion_overrides_item_id_module_items_id_fk" FOREIGN KEY ("item_id") REFERENCES "public"."module_items"("id") ON DELETE no action ON UPDATE no action;
