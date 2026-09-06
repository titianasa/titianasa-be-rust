CREATE TABLE "orders" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"enrollment_id" uuid NOT NULL,
	"amount_idr" bigint NOT NULL,
	"status" text DEFAULT 'pending' NOT NULL,
	"payment_id" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "orders_enrollment_id_unique" UNIQUE("enrollment_id"),
	CONSTRAINT "orders_status_check" CHECK ("orders"."status" in ('pending', 'paid', 'failed', 'refunded'))
);
--> statement-breakpoint
ALTER TABLE "orders" ADD CONSTRAINT "orders_enrollment_id_enrollments_id_fk" FOREIGN KEY ("enrollment_id") REFERENCES "public"."enrollments"("id") ON DELETE no action ON UPDATE no action;