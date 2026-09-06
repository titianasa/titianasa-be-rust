CREATE TABLE "certificates" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"enrollment_id" uuid NOT NULL,
	"certificate_code" text NOT NULL,
	"issued_at" timestamp with time zone DEFAULT now() NOT NULL,
	"completion_percent" integer NOT NULL,
	"estimated_cefr" text,
	"skill_summary" jsonb DEFAULT '{}'::jsonb NOT NULL,
	CONSTRAINT "certificates_enrollment_id_unique" UNIQUE("enrollment_id"),
	CONSTRAINT "certificates_certificate_code_unique" UNIQUE("certificate_code")
);
--> statement-breakpoint
ALTER TABLE "certificates" ADD CONSTRAINT "certificates_enrollment_id_enrollments_id_fk" FOREIGN KEY ("enrollment_id") REFERENCES "public"."enrollments"("id") ON DELETE no action ON UPDATE no action;