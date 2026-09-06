ALTER TABLE "cohorts" ADD COLUMN "meeting_url" text;--> statement-breakpoint
ALTER TABLE "enrollments" ADD COLUMN "sessions_remaining" integer;--> statement-breakpoint
ALTER TABLE "learning_products" ADD COLUMN "delivery_mode" text DEFAULT 'live_class' NOT NULL;--> statement-breakpoint
ALTER TABLE "learning_products" ADD COLUMN "session_count" integer;--> statement-breakpoint
ALTER TABLE "learning_products" ADD CONSTRAINT "learning_products_delivery_mode_check" CHECK ("learning_products"."delivery_mode" in ('live_class', 'self_paced', 'bootcamp', 'hybrid', 'package'));--> statement-breakpoint
ALTER TABLE "learning_products" ADD CONSTRAINT "learning_products_session_count_check" CHECK (("learning_products"."delivery_mode" = 'package' and "learning_products"."session_count" > 0) or ("learning_products"."delivery_mode" != 'package' and "learning_products"."session_count" is null));