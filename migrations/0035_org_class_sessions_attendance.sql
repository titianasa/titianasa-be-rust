-- migrations/0035_org_class_sessions_attendance.sql
-- Phase 35 — sessions + attendance for the org-owned `classes` domain
-- (Phase 32), mirroring the marketplace's cohort-scoped class_sessions/
-- attendance_records/session_participant_records (0024/0010) in shape,
-- but FK'd to `classes` instead of `cohorts` and gated through
-- org_class.rs's assert_can_manage_class instead of
-- cohort.product_id -> learning_products.tutor_id. See phase-35 ticket
-- for why this is a new set of tables rather than a retrofit of the
-- marketplace ones (same reasoning Phase 32 already used for `classes`
-- itself). Two deliberate differences from the marketplace shape:
-- session_date is a real `date` here (marketplace used `text`, no
-- reason to repeat that), and attendance status is the Indonesian
-- school-attendance vocabulary the user actually asked for (hadir/
-- alpha/sakit/izin/telat), not present/absent/late/excused. "Belum
-- diabsen" needs no status value — it's just the absence of a row.
CREATE TABLE "org_class_sessions" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"class_id" uuid NOT NULL,
	"session_date" date NOT NULL,
	"scheduled_start" timestamp with time zone NOT NULL,
	"scheduled_end" timestamp with time zone NOT NULL,
	"meeting_provider" text DEFAULT 'stub' NOT NULL,
	"external_meeting_id" text,
	"join_url" text,
	"status" text DEFAULT 'scheduled' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "org_class_sessions_provider_check" CHECK ("org_class_sessions"."meeting_provider" in ('stub', 'google_meet', 'zoom')),
	CONSTRAINT "org_class_sessions_status_check" CHECK ("org_class_sessions"."status" in ('scheduled', 'completed', 'cancelled'))
);
--> statement-breakpoint
CREATE INDEX "org_class_sessions_class_idx" ON "org_class_sessions" ("class_id");
--> statement-breakpoint

CREATE TABLE "org_session_participant_records" (
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
	CONSTRAINT "org_session_participant_records_role_check" CHECK ("org_session_participant_records"."role" in ('teacher', 'student')),
	CONSTRAINT "org_session_participant_records_status_check" CHECK ("org_session_participant_records"."verification_status" in ('present', 'late', 'partial', 'absent'))
);
--> statement-breakpoint

CREATE TABLE "org_attendance_records" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"class_id" uuid NOT NULL,
	"student_id" uuid NOT NULL,
	"session_date" date NOT NULL,
	"status" text NOT NULL,
	"method" text DEFAULT 'manual' NOT NULL,
	"marked_by" uuid,
	"marked_at" timestamp with time zone DEFAULT now() NOT NULL,
	"class_session_id" uuid,
	CONSTRAINT "org_attendance_records_class_student_date_key" UNIQUE("class_id","student_id","session_date"),
	CONSTRAINT "org_attendance_records_status_check" CHECK ("org_attendance_records"."status" in ('hadir', 'alpha', 'sakit', 'izin', 'telat')),
	CONSTRAINT "org_attendance_records_method_check" CHECK ("org_attendance_records"."method" in ('manual', 'qr_teacher', 'qr_student', 'google_meet'))
);
--> statement-breakpoint

ALTER TABLE "org_class_sessions" ADD CONSTRAINT "org_class_sessions_class_id_classes_id_fk" FOREIGN KEY ("class_id") REFERENCES "public"."classes"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "org_session_participant_records" ADD CONSTRAINT "org_session_participant_records_class_session_id_fk" FOREIGN KEY ("class_session_id") REFERENCES "public"."org_class_sessions"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "org_session_participant_records" ADD CONSTRAINT "org_session_participant_records_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "org_attendance_records" ADD CONSTRAINT "org_attendance_records_class_id_classes_id_fk" FOREIGN KEY ("class_id") REFERENCES "public"."classes"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "org_attendance_records" ADD CONSTRAINT "org_attendance_records_student_id_users_id_fk" FOREIGN KEY ("student_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "org_attendance_records" ADD CONSTRAINT "org_attendance_records_marked_by_users_id_fk" FOREIGN KEY ("marked_by") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;
--> statement-breakpoint
ALTER TABLE "org_attendance_records" ADD CONSTRAINT "org_attendance_records_class_session_id_fk" FOREIGN KEY ("class_session_id") REFERENCES "public"."org_class_sessions"("id") ON DELETE no action ON UPDATE no action;
