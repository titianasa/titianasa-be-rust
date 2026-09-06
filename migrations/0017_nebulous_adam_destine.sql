CREATE TABLE "tutor_reviews" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"tutor_id" uuid NOT NULL,
	"student_id" uuid NOT NULL,
	"enrollment_id" uuid NOT NULL,
	"rating" integer NOT NULL,
	"comment" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "tutor_reviews_enrollment_id_unique" UNIQUE("enrollment_id"),
	CONSTRAINT "tutor_reviews_rating_check" CHECK ("tutor_reviews"."rating" >= 1 and "tutor_reviews"."rating" <= 5)
);
--> statement-breakpoint
ALTER TABLE "tutor_reviews" ADD CONSTRAINT "tutor_reviews_tutor_id_users_id_fk" FOREIGN KEY ("tutor_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "tutor_reviews" ADD CONSTRAINT "tutor_reviews_student_id_users_id_fk" FOREIGN KEY ("student_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "tutor_reviews" ADD CONSTRAINT "tutor_reviews_enrollment_id_enrollments_id_fk" FOREIGN KEY ("enrollment_id") REFERENCES "public"."enrollments"("id") ON DELETE no action ON UPDATE no action;