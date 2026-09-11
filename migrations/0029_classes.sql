-- migrations/0029_classes.sql
-- Phase 32 (P32-002) — a new, minimal, org-owned "class" (roster of
-- students taught by one org `teacher`, optionally tied to a
-- module/program). Deliberately NOT built on the marketplace's
-- `cohorts`/`class_sessions`/`enrollments` — every layer of that
-- system resolves permissions by chasing cohort.product_id ->
-- learning_products.tutor_id (confirmed by reading cohort.rs and
-- class_session.rs's assert_can_manage_cohort), i.e. a cohort IS a
-- batch of a paid product. Retrofitting that for a no-purchase,
-- org-internal roster would mean making product_id nullable and
-- auditing every consumer across 6 service files — too much risk to a
-- live, money-adjacent feature for this v1. Session-scheduling and
-- attendance for these classes are a deliberate, deferred fast-follow.
CREATE TABLE "classes" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"organization_id" uuid NOT NULL,
	"teacher_id" uuid NOT NULL,
	"module_id" uuid,
	"program_id" uuid,
	"name" text NOT NULL,
	"description" text,
	"status" text DEFAULT 'active' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "classes_status_check" CHECK ("status" IN ('active', 'archived'))
);
--> statement-breakpoint
CREATE INDEX "classes_org_idx" ON "classes" ("organization_id");
--> statement-breakpoint
CREATE INDEX "classes_teacher_idx" ON "classes" ("teacher_id");
--> statement-breakpoint

CREATE TABLE "class_members" (
	"class_id" uuid NOT NULL,
	"student_id" uuid NOT NULL,
	"joined_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "class_members_pk" PRIMARY KEY("class_id", "student_id")
);
--> statement-breakpoint

ALTER TABLE "classes" ADD CONSTRAINT "classes_organization_id_organizations_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organizations"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "classes" ADD CONSTRAINT "classes_teacher_id_users_id_fk" FOREIGN KEY ("teacher_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "classes" ADD CONSTRAINT "classes_module_id_modules_id_fk" FOREIGN KEY ("module_id") REFERENCES "public"."modules"("id") ON DELETE set null ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "classes" ADD CONSTRAINT "classes_program_id_programs_id_fk" FOREIGN KEY ("program_id") REFERENCES "public"."programs"("id") ON DELETE set null ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "class_members" ADD CONSTRAINT "class_members_class_id_classes_id_fk" FOREIGN KEY ("class_id") REFERENCES "public"."classes"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "class_members" ADD CONSTRAINT "class_members_student_id_users_id_fk" FOREIGN KEY ("student_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;
