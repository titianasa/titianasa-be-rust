-- migrations/0025_module_system.sql
-- Phase 31 (P31-002) — ParaLabs-style nested module system, replacing
-- the rigid curricula/levels/units/lessons hierarchy. See
-- agent/docs/tickets/phase-31-module-system.md for the full design
-- rationale. This migration only ADDS new tables; the old hierarchy is
-- dropped in 0026_module_system_retarget.sql (run after this one).

-- Flat, student/enrollment-facing container. Replaces "curricula" 1:1
-- in role. `framework` deliberately has no CHECK — the old
-- cefr/cambridge/kurikulum_id/custom enum was English-specific and the
-- whole point of this migration is "any subject."
CREATE TABLE "programs" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"subject_id" uuid NOT NULL,
	"code" text NOT NULL,
	"name" text NOT NULL,
	"description" text,
	"framework" text,
	"version" integer DEFAULT 1 NOT NULL,
	"status" text DEFAULT 'draft' NOT NULL,
	"created_by" uuid,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "programs_code_unique" UNIQUE("code"),
	CONSTRAINT "programs_status_check" CHECK ("status" IN ('draft','published','archived'))
);
--> statement-breakpoint

-- Self-nesting module TREE. A "folder" and a "module" are the same row,
-- distinguished by is_folder — mirrors ParaLabs' org_modules exactly
-- (a "folder" there is just a module row with is_folder=true, reusing
-- the same parent_id column for both folder-of-folders and
-- folder-of-modules nesting). Replaces levels+units as a concept.
CREATE TABLE "modules" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"parent_id" uuid,
	"is_folder" boolean DEFAULT false NOT NULL,
	"subject_id" uuid,
	"code" text,
	"title" text NOT NULL,
	"description" text,
	"status" text DEFAULT 'draft' NOT NULL,
	"version" integer DEFAULT 1 NOT NULL,
	"order_index" integer DEFAULT 0 NOT NULL,
	"generated_by" text DEFAULT 'human' NOT NULL,
	"created_by" uuid,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "modules_status_check" CHECK ("status" IN ('draft','published','archived')),
	CONSTRAINT "modules_generated_by_check" CHECK ("generated_by" IN ('human','ai')),
	CONSTRAINT "modules_not_own_parent" CHECK ("parent_id" IS NULL OR "parent_id" <> "id"),
	CONSTRAINT "modules_subject_required_unless_folder" CHECK ("is_folder" OR "subject_id" IS NOT NULL)
);
--> statement-breakpoint
CREATE UNIQUE INDEX "modules_code_unique" ON "modules" ("code") WHERE "code" IS NOT NULL;
--> statement-breakpoint
CREATE INDEX "modules_parent_idx" ON "modules" ("parent_id");
--> statement-breakpoint

-- Reuse: the same module can be attached to N programs.
CREATE TABLE "program_modules" (
	"program_id" uuid NOT NULL,
	"module_id" uuid NOT NULL,
	"order_index" integer DEFAULT 0 NOT NULL,
	"label_override" text,
	CONSTRAINT "program_modules_pk" PRIMARY KEY("program_id","module_id")
);
--> statement-breakpoint

-- Prerequisites live on the module graph itself (portable across every
-- program that reuses the module), NOT on program_modules — mirrors
-- concept_prerequisites' exact shape (composite PK, self-referencing).
CREATE TABLE "module_prerequisites" (
	"module_id" uuid NOT NULL,
	"prerequisite_module_id" uuid NOT NULL,
	CONSTRAINT "module_prerequisites_pk" PRIMARY KEY("module_id","prerequisite_module_id")
);
--> statement-breakpoint

-- The SECOND, independent tree — content items WITHIN exactly one
-- module. node_type distinguishes organizational nodes ('section') from
-- actual gradable/publishable leaves ('item'). depth is denormalized
-- (parent's depth+1, 0 for roots) so the <=5-level rule is a cheap
-- CHECK instead of a WITH RECURSIVE count on every insert. Replaces
-- units(as organizer)+lessons(as leaf).
CREATE TABLE "module_items" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"module_id" uuid NOT NULL,
	"parent_id" uuid,
	"node_type" text NOT NULL,
	"depth" integer DEFAULT 0 NOT NULL,
	"title" text NOT NULL,
	"order_index" integer DEFAULT 0 NOT NULL,
	"content_type" text,
	"status" text DEFAULT 'draft' NOT NULL,
	"version" integer DEFAULT 1 NOT NULL,
	"superseded_by" uuid,
	"qa_report" jsonb,
	"generated_by" text DEFAULT 'human' NOT NULL,
	"created_by" uuid,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "module_items_node_type_check" CHECK ("node_type" IN ('section','item')),
	CONSTRAINT "module_items_depth_check" CHECK ("depth" >= 0 AND "depth" <= 4),
	CONSTRAINT "module_items_content_type_check" CHECK ("content_type" IS NULL OR "content_type" IN ('learn','practice','speaking','writing','review','assessment')),
	CONSTRAINT "module_items_status_check" CHECK ("status" IN ('draft','in_review','published','archived')),
	CONSTRAINT "module_items_generated_by_check" CHECK ("generated_by" IN ('human','ai')),
	CONSTRAINT "module_items_not_own_parent" CHECK ("parent_id" IS NULL OR "parent_id" <> "id")
);
--> statement-breakpoint
CREATE INDEX "module_items_tree_idx" ON "module_items" ("module_id","parent_id","order_index");
--> statement-breakpoint

-- Renamed from lesson_concepts, identical shape.
CREATE TABLE "module_item_concepts" (
	"item_id" uuid NOT NULL,
	"concept_id" uuid NOT NULL,
	"weight" double precision DEFAULT 1 NOT NULL,
	CONSTRAINT "module_item_concepts_pk" PRIMARY KEY("item_id","concept_id")
);
--> statement-breakpoint

-- Foreign keys (added after every table exists, same convention as 0000_flaky_bucky.sql).
ALTER TABLE "programs" ADD CONSTRAINT "programs_subject_id_subjects_id_fk" FOREIGN KEY ("subject_id") REFERENCES "public"."subjects"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "programs" ADD CONSTRAINT "programs_created_by_users_id_fk" FOREIGN KEY ("created_by") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "modules" ADD CONSTRAINT "modules_parent_id_modules_id_fk" FOREIGN KEY ("parent_id") REFERENCES "public"."modules"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "modules" ADD CONSTRAINT "modules_subject_id_subjects_id_fk" FOREIGN KEY ("subject_id") REFERENCES "public"."subjects"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "modules" ADD CONSTRAINT "modules_created_by_users_id_fk" FOREIGN KEY ("created_by") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "program_modules" ADD CONSTRAINT "program_modules_program_id_programs_id_fk" FOREIGN KEY ("program_id") REFERENCES "public"."programs"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "program_modules" ADD CONSTRAINT "program_modules_module_id_modules_id_fk" FOREIGN KEY ("module_id") REFERENCES "public"."modules"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_prerequisites" ADD CONSTRAINT "module_prerequisites_module_id_modules_id_fk" FOREIGN KEY ("module_id") REFERENCES "public"."modules"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_prerequisites" ADD CONSTRAINT "module_prerequisites_prerequisite_module_id_modules_id_fk" FOREIGN KEY ("prerequisite_module_id") REFERENCES "public"."modules"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_items" ADD CONSTRAINT "module_items_module_id_modules_id_fk" FOREIGN KEY ("module_id") REFERENCES "public"."modules"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_items" ADD CONSTRAINT "module_items_parent_id_module_items_id_fk" FOREIGN KEY ("parent_id") REFERENCES "public"."module_items"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_items" ADD CONSTRAINT "module_items_superseded_by_module_items_id_fk" FOREIGN KEY ("superseded_by") REFERENCES "public"."module_items"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_items" ADD CONSTRAINT "module_items_created_by_users_id_fk" FOREIGN KEY ("created_by") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_item_concepts" ADD CONSTRAINT "module_item_concepts_item_id_module_items_id_fk" FOREIGN KEY ("item_id") REFERENCES "public"."module_items"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "module_item_concepts" ADD CONSTRAINT "module_item_concepts_concept_id_concepts_id_fk" FOREIGN KEY ("concept_id") REFERENCES "public"."concepts"("id") ON DELETE cascade ON UPDATE no action;
