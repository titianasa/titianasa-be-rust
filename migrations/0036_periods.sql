-- migrations/0036_periods.sql
-- Phase 36 — "Periode": a generic, flexibly-named academic time period
-- an org tags its classes with. Deliberately NOT a rigid
-- semester/batch-specific model — the user's own ask ("batch atau
-- semester") treated the two as interchangeable, and structurally
-- they're not quite the same thing (batch/angkatan is normally a
-- student property; semester is normally a class property). A plain
-- name + optional date range lets an org call it either, without this
-- schema forcing one academic calendar shape.
CREATE TABLE "periods" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"organization_id" uuid NOT NULL,
	"name" text NOT NULL,
	"start_date" date,
	"end_date" date,
	"status" text DEFAULT 'active' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "periods_status_check" CHECK ("periods"."status" in ('active', 'archived'))
);
--> statement-breakpoint
CREATE INDEX "periods_org_idx" ON "periods" ("organization_id");
--> statement-breakpoint
ALTER TABLE "periods" ADD CONSTRAINT "periods_organization_id_organizations_id_fk" FOREIGN KEY ("organization_id") REFERENCES "public"."organizations"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint

ALTER TABLE "classes" ADD COLUMN "period_id" uuid;
--> statement-breakpoint
ALTER TABLE "classes" ADD CONSTRAINT "classes_period_id_periods_id_fk" FOREIGN KEY ("period_id") REFERENCES "public"."periods"("id") ON DELETE set null ON UPDATE no action;
