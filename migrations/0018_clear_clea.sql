ALTER TABLE "proctoring_events" ADD COLUMN "purged_at" timestamp with time zone;--> statement-breakpoint
ALTER TABLE "proctoring_policies" ADD COLUMN "retention_days" integer DEFAULT 30 NOT NULL;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD COLUMN "consent_given_at" timestamp with time zone NOT NULL;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD COLUMN "review_status" text DEFAULT 'pending' NOT NULL;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD COLUMN "reviewed_by" uuid;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD COLUMN "reviewed_at" timestamp with time zone;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD COLUMN "review_notes" text;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD CONSTRAINT "proctoring_sessions_reviewed_by_users_id_fk" FOREIGN KEY ("reviewed_by") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "proctoring_sessions" ADD CONSTRAINT "proctoring_sessions_review_status_check" CHECK ("proctoring_sessions"."review_status" in ('pending', 'cleared', 'flagged', 'violation_confirmed'));