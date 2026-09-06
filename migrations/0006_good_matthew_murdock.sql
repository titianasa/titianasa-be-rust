CREATE TABLE "user_daily_missions" (
	"user_id" uuid NOT NULL,
	"mission_date" text NOT NULL,
	"progress" jsonb DEFAULT '{}'::jsonb NOT NULL,
	"reward_claimed" boolean DEFAULT false NOT NULL,
	CONSTRAINT "user_daily_missions_user_id_mission_date_pk" PRIMARY KEY("user_id","mission_date")
);
--> statement-breakpoint
ALTER TABLE "user_daily_missions" ADD CONSTRAINT "user_daily_missions_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE no action ON UPDATE no action;