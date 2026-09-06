CREATE TABLE "learning_products" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"tutor_id" uuid NOT NULL,
	"type" text NOT NULL,
	"title" text NOT NULL,
	"description" text DEFAULT '' NOT NULL,
	"price_idr" bigint NOT NULL,
	"capacity" integer,
	"status" text DEFAULT 'draft' NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "learning_products_type_check" CHECK ("learning_products"."type" in ('private', 'group')),
	CONSTRAINT "learning_products_status_check" CHECK ("learning_products"."status" in ('draft', 'published', 'archived')),
	CONSTRAINT "learning_products_price_check" CHECK ("learning_products"."price_idr" >= 0),
	CONSTRAINT "learning_products_capacity_by_type_check" CHECK (("learning_products"."type" = 'private' and "learning_products"."capacity" is null) or ("learning_products"."type" = 'group' and "learning_products"."capacity" > 0))
);
--> statement-breakpoint
ALTER TABLE "learning_products" ADD CONSTRAINT "learning_products_tutor_id_tutor_profiles_user_id_fk" FOREIGN KEY ("tutor_id") REFERENCES "public"."tutor_profiles"("user_id") ON DELETE no action ON UPDATE no action;