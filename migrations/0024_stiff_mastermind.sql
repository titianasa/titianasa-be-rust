CREATE TABLE "class_sessions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"cohort_id" uuid NOT NULL,
	"session_date" text NOT NULL,
	"scheduled_start" timestamp with time zone NOT NULL,
	"scheduled_end" timestamp with time zone NOT NULL,
	"meeting_provider" text DEFAULT 'stub' NOT NULL,
	"external_meeting_id" text,
	"join_url" text,
	"status" text DEFAULT 'scheduled' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "class_sessions_provider_check" CHECK ("class_sessions"."meeting_provider" in ('stub', 'google_meet', 'zoom')),
	CONSTRAINT "class_sessions_status_check" CHECK ("class_sessions"."status" in ('scheduled', 'completed', 'cancelled'))
);
--> statement-breakpoint
CREATE TABLE "session_participant_records" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"class_session_id" uuid NOT NULL,
	"user_id" uuid,
	"role" text NOT NULL,
	"external_participant_name" text,
	"external_google_account_id" text,
	"first_joined_at" timestamp with time zone NOT NULL,
	"last_left_at" timestamp with time zone NOT NULL,
	"duration_seconds" integer DEFAULT 0 NOT NULL,
	"join_session_count" integer DEFAULT 1 NOT NULL,
	"verification_status" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "session_participant_records_role_check" CHECK ("session_participant_records"."role" in ('tutor', 'student')),
	CONSTRAINT "session_participant_records_status_check" CHECK ("session_participant_records"."verification_status" in ('present', 'late', 'partial', 'absent'))
);
--> statement-breakpoint
ALTER TABLE "attendance_records" DROP CONSTRAINT "attendance_records_status_check";--> statement-breakpoint
ALTER TABLE "attendance_records" DROP CONSTRAINT "attendance_records_method_check";--> statement-breakpoint
ALTER TABLE "attendance_records" ALTER COLUMN "marked_by" DROP NOT NULL;--> statement-breakpoint
ALTER TABLE "attendance_records" ADD COLUMN "class_session_id" uuid;--> statement-breakpoint
ALTER TABLE "class_sessions" ADD CONSTRAINT "class_sessions_cohort_id_cohorts_id_fk" FOREIGN KEY ("cohort_id") REFERENCES "public"."cohorts"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "session_participant_records" ADD CONSTRAINT "session_participant_records_class_session_id_class_sessions_id_fk" FOREIGN KEY ("class_session_id") REFERENCES "public"."class_sessions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "session_participant_records" ADD CONSTRAINT "session_participant_records_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "idx_class_sessions_cohort" ON "class_sessions" USING btree ("cohort_id");--> statement-breakpoint
CREATE INDEX "idx_session_participant_records_session" ON "session_participant_records" USING btree ("class_session_id");--> statement-breakpoint
ALTER TABLE "attendance_records" ADD CONSTRAINT "attendance_records_class_session_id_class_sessions_id_fk" FOREIGN KEY ("class_session_id") REFERENCES "public"."class_sessions"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "attendance_records" ADD CONSTRAINT "attendance_records_status_check" CHECK ("attendance_records"."status" in ('present', 'absent', 'late', 'excused', 'partial'));--> statement-breakpoint
ALTER TABLE "attendance_records" ADD CONSTRAINT "attendance_records_method_check" CHECK ("attendance_records"."method" in ('manual', 'online'));